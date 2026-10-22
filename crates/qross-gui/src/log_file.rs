//! Protokolldatei `qross.log` im Datenordner: Was Qross tat, auch ohne Fenster, etwa beim Start mit
//! der Anmeldung. Wird sie zu groß, beginnt eine neue, die vorige bleibt als `qross.old.log`.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

/// Protokolldatei im Datenordner: Was Qross auch ohne Fenster tat, etwa beim Start mit der Anmeldung.
pub const LOG_FILE: &str = "qross.log";
/// Die vorige Datei, wenn die aktuelle `MAX_FILE_BYTES` erreicht hat
pub const OLD_LOG_FILE: &str = "qross.old.log";
const MAX_FILE_BYTES: u64 = 1024 * 1024;

/// Schreibt in `qross.log` und beginnt eine neue Datei, sobald sie zu groß wird; die alte bleibt als
/// `qross.old.log`. Zusammen also höchstens etwa 2 MB.
pub struct LogFile {
    dir: PathBuf,
    file: Option<File>,
    written: u64,
}

impl LogFile {
    pub fn open(dir: &Path) -> Self {
        let mut log = Self { dir: dir.to_path_buf(), file: None, written: 0 };
        log.reopen();
        log
    }

    fn reopen(&mut self) {
        let path = self.dir.join(LOG_FILE);
        if fs::metadata(&path).is_ok_and(|meta| meta.len() >= MAX_FILE_BYTES) {
            let _ = fs::rename(&path, self.dir.join(OLD_LOG_FILE));
        }
        let _ = fs::create_dir_all(&self.dir);
        self.file = OpenOptions::new().create(true).append(true).open(&path).ok();
        self.written = self.file.as_ref().and_then(|f| f.metadata().ok()).map_or(0, |meta| meta.len());
    }
}

impl io::Write for LogFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.written >= MAX_FILE_BYTES {
            self.file = None;
            self.reopen();
        }
        // Ohne Datei (Ordner nicht beschreibbar) geht die Zeile verloren, nicht das Programm
        let Some(file) = self.file.as_mut() else { return Ok(buf.len()) };
        let n = file.write(buf)?;
        self.written += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.as_mut().map_or(Ok(()), File::flush)
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    #[test]
    fn the_log_file_starts_over_when_it_is_full() {
        let dir = std::env::temp_dir().join(format!("qross-log-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let mut log = LogFile::open(&dir);
        let line = [b'x'; 1000];
        for _ in 0..(MAX_FILE_BYTES / 1000 + 1) {
            log.write_all(&line).unwrap();
        }
        log.write_all(b"neu\n").unwrap();
        log.flush().unwrap();
        assert_eq!(fs::read(dir.join(LOG_FILE)).unwrap(), b"neu\n", "neue Datei beginnt");
        assert!(fs::metadata(dir.join(OLD_LOG_FILE)).unwrap().len() >= MAX_FILE_BYTES, "die volle bleibt als alte");
        // Ein neuer Start hängt an, bis die Datei voll ist
        drop(log);
        let mut again = LogFile::open(&dir);
        again.write_all(b"zweiter Start\n").unwrap();
        assert_eq!(fs::read_to_string(dir.join(LOG_FILE)).unwrap(), "neu\nzweiter Start\n");
        let _ = fs::remove_dir_all(&dir);
    }
}
