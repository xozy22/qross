//! Kopierte Dateien auf Abruf (siehe [`crate::Clipboard::write_virtual_files`]): In der Zwischenablage
//! stehen nur Namen und Größen; die Inhalte liefert eine [`VirtualSource`] erst, wenn ein Programm die
//! Dateien einfügt. Unter Windows sind das „virtuelle Dateien“ (`FileGroupDescriptorW` und
//! `FileContents`), wie sie auch Outlook für Anhänge oder Remotedesktop ablegen.

use std::fmt;
use std::io;
use std::sync::Arc;

use anyhow::bail;
use qross_proto::FileEntry;

/// Dateien für [`crate::Clipboard::write_virtual_files`]
#[derive(Clone)]
pub struct VirtualFiles {
    /// Relative Pfade wie `FileEntry::path` (mit `/` getrennt, schon bereinigt), Ordner vor ihrem
    /// Inhalt. Die Stelle in der Liste ist der Index für [`VirtualSource::open`].
    pub entries: Vec<FileEntry>,
    pub source: Arc<dyn VirtualSource>,
}

impl fmt::Debug for VirtualFiles {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VirtualFiles").field("entries", &self.entries).finish_non_exhaustive()
    }
}

/// Liefert die Inhalte, sobald ein Programm einfügt. Wird aus Systemthreads aufgerufen, auch
/// gleichzeitig (zweimal eingefügt).
pub trait VirtualSource: Send + Sync {
    /// Ein Programm will Datei `index` lesen (Windows: `GetData` mit `FileContents`). Wartet nicht;
    /// geholt wird erst beim Lesen.
    fn open(&self, index: usize) -> Box<dyn VirtualFile>;
}

/// Eine zum Lesen geöffnete virtuelle Datei. Verworfen wird sie, wenn das Programm nicht mehr liest:
/// fertig oder abgebrochen.
pub trait VirtualFile: Send {
    /// Liest ab `offset` bis zu `buf.len()` Bytes und wartet, bis welche da sind; `0` am Ende der Datei.
    /// Ein Abbruch (hier oder beim Sender) kommt als Fehler, für den [`is_cancelled`] gilt.
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<usize>;
}

/// Das Einfügen wurde abgebrochen (siehe [`cancelled`])
#[derive(Debug)]
struct Cancelled;

impl fmt::Display for Cancelled {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("paste cancelled")
    }
}

impl std::error::Error for Cancelled {}

/// Fehler für [`VirtualFile::read_at`], wenn das Einfügen abgebrochen wurde. Windows meldet dem
/// Programm dann „abgebrochen“ statt eines Lesefehlers; der Explorer hört ohne Fehlermeldung auf.
pub fn cancelled() -> io::Error {
    io::Error::other(Cancelled)
}

/// Kam der Fehler von [`cancelled`]?
pub fn is_cancelled(error: &io::Error) -> bool {
    error.get_ref().is_some_and(|inner| inner.is::<Cancelled>())
}

/// Größe eines `FILEDESCRIPTORW`
const DESCRIPTOR_LEN: usize = 592;
/// Platz für den Pfad in `FILEDESCRIPTORW::cFileName` (`MAX_PATH`), samt Nullzeichen
const NAME_UNITS: usize = 260;
/// `FD_ATTRIBUTES | FD_FILESIZE | FD_PROGRESSUI`: Attribute und Größe gelten, der Explorer zeigt beim
/// Einfügen seinen Fortschritt.
const FD_FLAGS: u32 = 0x4 | 0x40 | 0x4000;
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
const FILE_ATTRIBUTE_NORMAL: u32 = 0x80;

/// Inhalt des Formats `FileGroupDescriptorW` für `entries` (Windows): die Anzahl und je Eintrag ein
/// `FILEDESCRIPTORW` mit dem relativen Pfad (`\` statt `/`), Größe und Attributen. Ein Pfad muss in
/// `MAX_PATH` passen, sonst kann der Explorer die Dateien nicht anlegen.
pub fn file_group_descriptor(entries: &[FileEntry]) -> anyhow::Result<Vec<u8>> {
    let mut out = Vec::with_capacity(4 + entries.len() * DESCRIPTOR_LEN);
    out.extend_from_slice(&u32::try_from(entries.len())?.to_le_bytes());
    for entry in entries {
        let name: Vec<u16> = entry.path.replace('/', "\\").encode_utf16().collect();
        if name.is_empty() || name.len() >= NAME_UNITS {
            bail!("path '{}' does not fit into a virtual file (at most {} characters)", entry.path, NAME_UNITS - 1);
        }
        let start = out.len();
        out.resize(start + DESCRIPTOR_LEN, 0);
        let descriptor = &mut out[start..];
        let attributes = if entry.is_dir { FILE_ATTRIBUTE_DIRECTORY } else { FILE_ATTRIBUTE_NORMAL };
        let size = if entry.is_dir { 0 } else { entry.size };
        descriptor[0..4].copy_from_slice(&FD_FLAGS.to_le_bytes());
        // clsid (4), sizel (20), pointl (28) bleiben leer.
        descriptor[36..40].copy_from_slice(&attributes.to_le_bytes());
        // Erstellt, Zugriff, Geändert (40, 48, 56) kennt die Gegenstelle nicht mit.
        descriptor[64..68].copy_from_slice(&((size >> 32) as u32).to_le_bytes());
        descriptor[68..72].copy_from_slice(&(size as u32).to_le_bytes());
        for (unit, slot) in name.iter().zip(descriptor[72..].as_chunks_mut::<2>().0) {
            *slot = unit.to_le_bytes();
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, size: u64, is_dir: bool) -> FileEntry {
        FileEntry { path: path.into(), size, is_dir }
    }

    fn u32_at(bytes: &[u8], at: usize) -> u32 {
        u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
    }

    fn name_at(bytes: &[u8], at: usize) -> String {
        let units: Vec<u16> =
            bytes[at..at + 2 * NAME_UNITS].as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).take_while(|&u| u != 0).collect();
        String::from_utf16(&units).unwrap()
    }

    #[test]
    fn the_descriptor_lists_folders_and_files_with_size() {
        let entries = [entry("Urlaub", 0, true), entry("Urlaub/Strand.mov", 4_700_000_000, false), entry("Grüße.txt", 5, false)];
        let bytes = file_group_descriptor(&entries).unwrap();
        assert_eq!(bytes.len(), 4 + 3 * DESCRIPTOR_LEN);
        assert_eq!(u32_at(&bytes, 0), 3);

        let folder = 4;
        assert_eq!(u32_at(&bytes, folder), FD_FLAGS);
        assert_eq!(u32_at(&bytes, folder + 36), FILE_ATTRIBUTE_DIRECTORY);
        assert_eq!(name_at(&bytes, folder + 72), "Urlaub");

        let film = folder + DESCRIPTOR_LEN;
        assert_eq!(u32_at(&bytes, film + 36), FILE_ATTRIBUTE_NORMAL);
        let size = (u64::from(u32_at(&bytes, film + 64)) << 32) | u64::from(u32_at(&bytes, film + 68));
        assert_eq!(size, 4_700_000_000, "über 4 GB: oberer und unterer Teil");
        assert_eq!(name_at(&bytes, film + 72), "Urlaub\\Strand.mov");

        assert_eq!(name_at(&bytes, film + DESCRIPTOR_LEN + 72), "Grüße.txt");
        assert!(file_group_descriptor(&[]).unwrap() == 0u32.to_le_bytes(), "leer: nur die Anzahl");
    }

    #[test]
    fn paths_must_fit_into_max_path() {
        let longest = "a/".repeat(129) + "b";
        assert_eq!(longest.len(), NAME_UNITS - 1);
        assert!(file_group_descriptor(&[entry(&longest, 1, false)]).is_ok());
        let error = file_group_descriptor(&[entry(&format!("{longest}c"), 1, false)]).unwrap_err();
        assert!(error.to_string().contains("at most 259 characters"), "{error}");
    }

    #[test]
    fn cancelling_is_recognized() {
        assert!(is_cancelled(&cancelled()));
        assert!(!is_cancelled(&io::Error::other("Verbindung getrennt")));
        assert!(!is_cancelled(&io::Error::from(io::ErrorKind::Interrupted)));
    }
}
