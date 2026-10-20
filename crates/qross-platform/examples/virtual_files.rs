//! Prüft kopierte Dateien auf Abruf mit der echten Zwischenablage dieses Rechners (nur Windows): Qross
//! legt virtuelle Dateien ab (Ordner mit Unterordner, eine größere und eine leere Datei), deren Inhalte
//! erst beim Lesen entstehen, und ein zweiter Prozess fügt sie wie Strg+V im Explorer in einen
//! Testordner ein. Danach dasselbe mit einem Abbruch mitten in der großen Datei. Der vorherige Inhalt der
//! Zwischenablage wird wiederhergestellt.
//!
//! `cargo run -p qross-platform --example virtual_files [MB]` (Größe der großen Datei, Standard 50)

#[cfg(not(windows))]
fn main() {
    println!("nur unter Windows");
}

#[cfg(windows)]
fn main() -> anyhow::Result<()> {
    windows_check::run()
}

#[cfg(windows)]
mod windows_check {
    use std::collections::BTreeSet;
    use std::io;
    use std::path::Path;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    use qross_platform::virtual_files::cancelled;
    use qross_platform::{VirtualFile, VirtualFiles, VirtualSource};
    use qross_proto::{ClipboardData, FileEntry};

    /// Inhalt, der sich aus der Position ergibt; so lässt sich jede Datei ohne Kopie prüfen.
    fn byte_at(seed: u64, at: u64) -> u8 {
        ((at.wrapping_add(seed).wrapping_mul(2_654_435_761)) >> 24) as u8
    }

    /// Liefert die Inhalte; merkt sich, in welchen Threads gelesen wurde.
    struct Source {
        entries: Vec<FileEntry>,
        /// Die große Datei nach so vielen Bytes abbrechen (`u64::MAX`: nie)
        cancel_after: u64,
        threads: Arc<Mutex<BTreeSet<String>>>,
        read: Arc<AtomicU64>,
    }

    struct File {
        seed: u64,
        size: u64,
        cancel_after: u64,
        threads: Arc<Mutex<BTreeSet<String>>>,
        read: Arc<AtomicU64>,
    }

    impl VirtualSource for Source {
        fn open(&self, index: usize) -> Box<dyn VirtualFile> {
            let size = self.entries[index].size;
            Box::new(File {
                seed: index as u64,
                size,
                cancel_after: if size > 1 << 20 { self.cancel_after } else { u64::MAX },
                threads: self.threads.clone(),
                read: self.read.clone(),
            })
        }
    }

    impl VirtualFile for File {
        fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
            let thread = std::thread::current().name().unwrap_or("ohne Namen (RPC)").to_owned();
            self.threads.lock().unwrap().insert(thread);
            if offset >= self.cancel_after {
                return Err(cancelled());
            }
            let n = self.size.saturating_sub(offset).min(buf.len() as u64).min(256 * 1024) as usize;
            for (i, b) in buf[..n].iter_mut().enumerate() {
                *b = byte_at(self.seed, offset + i as u64);
            }
            self.read.fetch_add(n as u64, Ordering::Relaxed);
            Ok(n)
        }
    }

    pub fn run() -> anyhow::Result<()> {
        if let (Some("einfuegen"), Some(folder)) = (std::env::args().nth(1).as_deref(), std::env::args().nth(2)) {
            return qross_platform::paste_clipboard_into(Path::new(&folder));
        }
        let big_mb: u64 = std::env::args().nth(1).map(|a| a.parse()).transpose()?.unwrap_or(50);
        let mut clipboard = qross_platform::clipboard();
        let backup = clipboard.read();
        println!("fügt auf Abruf ein: {}", clipboard.pastes_on_demand());

        let big = big_mb * 1_000_000;
        let entries = vec![
            FileEntry { path: "Qross Ordner Ä".into(), size: 0, is_dir: true },
            FileEntry { path: "Qross Ordner Ä/drin.txt".into(), size: 9, is_dir: false },
            FileEntry { path: "Qross Ordner Ä/Unterordner".into(), size: 0, is_dir: true },
            FileEntry { path: "Qross Groß.bin".into(), size: big, is_dir: false },
            FileEntry { path: "leer.txt".into(), size: 0, is_dir: false },
        ];
        let dir = std::env::temp_dir().join(format!("qross-virtual-files-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        for (round, cancel_after) in [("vollständig", u64::MAX), ("mit Abbruch", big / 3)] {
            let threads = Arc::new(Mutex::new(BTreeSet::new()));
            let read = Arc::new(AtomicU64::new(0));
            let source = Source { entries: entries.clone(), cancel_after, threads: threads.clone(), read: read.clone() };
            let files = VirtualFiles { entries: entries.clone(), source: Arc::new(source) };
            clipboard.write_virtual_files(files, &ClipboardData::from_text("Qross Ordner Ä\nQross Groß.bin\nleer.txt"))?;
            println!("[{round}] Text daneben: {:?}", clipboard.text());

            let target = dir.join(round);
            std::fs::create_dir_all(&target)?;
            let started = Instant::now();
            let status = std::process::Command::new(std::env::current_exe()?).arg("einfuegen").arg(&target).status()?;
            let folder = target.join("Qross Ordner Ä");
            let inner = std::fs::read(folder.join("drin.txt")).ok();
            println!(
                "[{round}] {} nach {:.1} s: Ordner {}, Unterordner {}, drin.txt {}, große Datei {}, leer.txt {}",
                if status.success() { "eingefügt" } else { "Einfügen gescheitert" },
                started.elapsed().as_secs_f64(),
                folder.is_dir(),
                folder.join("Unterordner").is_dir(),
                inner.map_or("fehlt".into(), |b| check_bytes(&b, 1, 9)),
                std::fs::read(target.join("Qross Groß.bin")).map_or("fehlt".into(), |b| check_bytes(&b, 3, big)),
                target.join("leer.txt").exists(),
            );
            println!("[{round}] gelesen: {:.1} MB, in Threads: {:?}", read.load(Ordering::Relaxed) as f64 / 1e6, threads.lock().unwrap());
        }
        let _ = std::fs::remove_dir_all(&dir);
        match backup {
            Some(previous) => clipboard.write(&previous)?,
            None => clipboard.write(&ClipboardData::default())?,
        }
        println!("vorheriger Inhalt wiederhergestellt");
        Ok(())
    }

    /// Größe und Inhalt einer eingefügten Datei
    fn check_bytes(bytes: &[u8], seed: u64, expected: u64) -> String {
        let same = bytes.iter().enumerate().all(|(i, b)| *b == byte_at(seed, i as u64));
        format!("{} von {expected} B ({})", bytes.len(), if same { "Inhalt richtig" } else { "Inhalt FALSCH" })
    }
}
