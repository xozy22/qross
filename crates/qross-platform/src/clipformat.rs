//! Formate der Zwischenablage umwandeln – ohne Betriebssystem-Aufrufe, daher überall testbar.
//!
//! - Bilder gehen als PNG übers Netz. Windows legt Bilder meist als DIB ab (Bitmap ohne
//!   Datei-Kopf), Linux/macOS liefern über `arboard` RGBA-Pixel.
//! - Windows speichert HTML im Format „CF_HTML“: UTF-8 mit einem Kopf, der Byte-Positionen des
//!   eigentlichen Ausschnitts enthält. Übertragen wird nur der Ausschnitt.
//! - Kopierte Dateien liegen unter Windows als „CF_HDROP“ vor: ein `DROPFILES`-Kopf und danach
//!   die Pfade, durch Nullzeichen getrennt.

use std::io::Cursor;
use std::path::{Path, PathBuf};

use anyhow::{bail, ensure, Context, Result};

/// RGBA-Bild, 8 Bit je Kanal, Zeilen von oben nach unten
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rgba {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

pub fn rgba_to_png(image: &Rgba) -> Result<Vec<u8>> {
    ensure!(image.pixels.len() == image.width as usize * image.height as usize * 4, "pixel count does not match the size");
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, image.width, image.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    // Schnell statt maximal klein: Bildschirmfotos sollen ohne Verzögerung rüber.
    encoder.set_compression(png::Compression::Fast);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(&image.pixels)?;
    writer.finish()?;
    Ok(out)
}

pub fn png_to_rgba(data: &[u8]) -> Result<Rgba> {
    let mut decoder = png::Decoder::new(Cursor::new(data));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().context("not a valid PNG")?;
    let mut buf = vec![0; reader.output_buffer_size().context("PNG too large")?];
    let info = reader.next_frame(&mut buf)?;
    buf.truncate(info.buffer_size());
    let pixels = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => buf.as_chunks::<3>().0.iter().flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        png::ColorType::GrayscaleAlpha => buf.as_chunks::<2>().0.iter().flat_map(|p| [p[0], p[0], p[0], p[1]]).collect(),
        png::ColorType::Grayscale => buf.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => bail!("PNG palette was not expanded"),
    };
    Ok(Rgba { width: info.width, height: info.height, pixels })
}

fn u16_at(b: &[u8], at: usize) -> Result<u16> {
    Ok(u16::from_le_bytes(b.get(at..at + 2).context("DIB too short")?.try_into()?))
}

fn u32_at(b: &[u8], at: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(b.get(at..at + 4).context("DIB too short")?.try_into()?))
}

const BI_RGB: u32 = 0;
const BI_BITFIELDS: u32 = 3;
const BI_ALPHABITFIELDS: u32 = 6;

/// Liest ein Windows-DIB (CF_DIB/CF_DIBV5: BITMAPINFOHEADER, -V4 oder -V5 plus Pixel).
/// Unterstützt 24 und 32 Bit, unkomprimiert oder mit Bitmasken – das deckt Screenshots ab.
pub fn dib_to_rgba(dib: &[u8]) -> Result<Rgba> {
    let header = u32_at(dib, 0)? as usize;
    ensure!(header >= 40, "unknown DIB header ({header} bytes)");
    let width = i32::from_le_bytes(dib[4..8].try_into()?);
    let height = i32::from_le_bytes(dib[8..12].try_into()?);
    let bits = u16_at(dib, 14)?;
    let compression = u32_at(dib, 16)?;
    ensure!(width > 0 && height != 0, "invalid DIB size {width}×{height}");
    ensure!(bits == 24 || bits == 32, "{bits}-bit DIBs are not supported");
    let (w, h) = (width as usize, height.unsigned_abs() as usize);
    let top_down = height < 0;

    // Farbmasken: bei BI_BITFIELDS hinter einem 40-Byte-Kopf, bei V4/V5 im Kopf selbst
    let (mut masks, mut pixel_offset) = ([0x00FF_0000, 0x0000_FF00, 0x0000_00FF, 0xFF00_0000u32], header);
    if compression == BI_BITFIELDS || compression == BI_ALPHABITFIELDS {
        let count = if compression == BI_ALPHABITFIELDS { 4 } else { 3 };
        // Offset 40: direkt hinter dem 40-Byte-Kopf bzw. an derselben Stelle im V4/V5-Kopf
        for (i, mask) in masks.iter_mut().enumerate().take(count) {
            *mask = u32_at(dib, 40 + 4 * i)?;
        }
        if header == 40 {
            pixel_offset += 4 * count;
        }
        if header >= 56 && compression == BI_BITFIELDS {
            masks[3] = u32_at(dib, 52)?;
        }
    } else {
        ensure!(compression == BI_RGB, "compressed DIBs (type {compression}) are not supported");
    }

    let bytes_per_pixel = bits as usize / 8;
    let stride = (w * bytes_per_pixel).div_ceil(4) * 4;
    let data = dib.get(pixel_offset..pixel_offset + stride * h).context("DIB pixel data incomplete")?;
    let channel = |px: u32, mask: u32| -> u8 {
        if mask == 0 {
            return 255;
        }
        let shift = mask.trailing_zeros();
        let max = mask >> shift;
        (((px & mask) >> shift) * 255 / max) as u8
    };

    let mut pixels = vec![0u8; w * h * 4];
    let mut any_alpha = false;
    for y in 0..h {
        let src_row = if top_down { y } else { h - 1 - y };
        let row = &data[src_row * stride..src_row * stride + w * bytes_per_pixel];
        for x in 0..w {
            let out = &mut pixels[(y * w + x) * 4..][..4];
            if bits == 24 {
                let p = &row[x * 3..x * 3 + 3];
                out.copy_from_slice(&[p[2], p[1], p[0], 255]);
            } else {
                let px = u32::from_le_bytes(row[x * 4..x * 4 + 4].try_into()?);
                let a = channel(px, masks[3]);
                any_alpha |= a != 0;
                out.copy_from_slice(&[channel(px, masks[0]), channel(px, masks[1]), channel(px, masks[2]), a]);
            }
        }
    }
    // Viele Programme legen 32-Bit-Bilder mit Alpha 0 ab, meinen aber „undurchsichtig“.
    if bits == 32 && !any_alpha {
        pixels.as_chunks_mut::<4>().0.iter_mut().for_each(|p| p[3] = 255);
    }
    Ok(Rgba { width: w as u32, height: h as u32, pixels })
}

/// Erzeugt ein CF_DIB (BITMAPINFOHEADER, 32 Bit BGRA, Zeilen von unten nach oben).
pub fn rgba_to_dib(image: &Rgba) -> Vec<u8> {
    let (w, h) = (image.width as usize, image.height as usize);
    let mut dib = Vec::with_capacity(40 + w * h * 4);
    dib.extend_from_slice(&40u32.to_le_bytes());
    dib.extend_from_slice(&(image.width as i32).to_le_bytes());
    dib.extend_from_slice(&(image.height as i32).to_le_bytes());
    dib.extend_from_slice(&1u16.to_le_bytes()); // Ebenen
    dib.extend_from_slice(&32u16.to_le_bytes()); // Bits je Pixel
    dib.extend_from_slice(&BI_RGB.to_le_bytes());
    dib.extend_from_slice(&((w * h * 4) as u32).to_le_bytes());
    dib.extend_from_slice(&[0u8; 16]); // Auflösung, Palette
    for y in (0..h).rev() {
        for p in image.pixels[y * w * 4..(y + 1) * w * 4].as_chunks::<4>().0 {
            dib.extend_from_slice(&[p[2], p[1], p[0], p[3]]);
        }
    }
    dib
}

/// Holt den HTML-Ausschnitt aus einem CF_HTML-Block.
pub fn cf_html_fragment(cf_html: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(cf_html);
    let offset = |key: &str| -> Option<usize> {
        let start = text.find(key)? + key.len();
        let digits: String = text[start..].chars().take_while(char::is_ascii_digit).collect();
        digits.parse().ok()
    };
    if let (Some(start), Some(end)) = (offset("StartFragment:"), offset("EndFragment:")) {
        if start <= end && end <= cf_html.len() {
            return Some(String::from_utf8_lossy(&cf_html[start..end]).into_owned());
        }
    }
    // Ersatzweise über die Kommentar-Marken
    let start = text.find("<!--StartFragment-->")? + "<!--StartFragment-->".len();
    let end = text[start..].find("<!--EndFragment-->")? + start;
    Some(text[start..end].to_owned())
}

/// Baut einen CF_HTML-Block um einen HTML-Ausschnitt.
pub fn build_cf_html(fragment: &str) -> Vec<u8> {
    const PREFIX: &str = "<html><body>\r\n<!--StartFragment-->";
    const SUFFIX: &str = "<!--EndFragment-->\r\n</body></html>";
    // Der Kopf hat feste Länge (10-stellige Zahlen), daher lassen sich die Positionen vorab berechnen.
    let header_len = "Version:0.9\r\nStartHTML:0000000000\r\nEndHTML:0000000000\r\nStartFragment:0000000000\r\nEndFragment:0000000000\r\n".len();
    let start_html = header_len;
    let start_fragment = start_html + PREFIX.len();
    let end_fragment = start_fragment + fragment.len();
    let end_html = end_fragment + SUFFIX.len();
    format!(
        "Version:0.9\r\nStartHTML:{start_html:010}\r\nEndHTML:{end_html:010}\r\n\
         StartFragment:{start_fragment:010}\r\nEndFragment:{end_fragment:010}\r\n{PREFIX}{fragment}{SUFFIX}"
    )
    .into_bytes()
}

/// Größe des `DROPFILES`-Kopfs: pFiles, pt.x, pt.y, fNC, fWide (je 4 Bytes)
const DROPFILES_LEN: usize = 20;

/// Pfade aus einem CF_HDROP-Block (UTF-16 oder, bei sehr alten Programmen, 8-Bit-Zeichen).
pub fn parse_dropfiles(block: &[u8]) -> Vec<PathBuf> {
    let (Ok(offset), Ok(wide)) = (u32_at(block, 0), u32_at(block, 16)) else { return Vec::new() };
    let Some(list) = block.get(offset as usize..) else { return Vec::new() };
    let names: Vec<String> = if wide != 0 {
        let units: Vec<u16> = list.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
        units.split(|&u| u == 0).take_while(|s| !s.is_empty()).map(String::from_utf16_lossy).collect()
    } else {
        list.split(|&b| b == 0).take_while(|s| !s.is_empty()).map(|s| String::from_utf8_lossy(s).into_owned()).collect()
    };
    names.into_iter().map(PathBuf::from).collect()
}

/// CF_HDROP-Block für absolute Pfade (UTF-16, doppelt nullterminiert).
pub fn build_dropfiles(paths: &[PathBuf]) -> Vec<u8> {
    let mut block = Vec::with_capacity(DROPFILES_LEN + paths.len() * 128);
    block.extend_from_slice(&(DROPFILES_LEN as u32).to_le_bytes());
    block.extend_from_slice(&[0u8; 12]); // pt, fNC
    block.extend_from_slice(&1u32.to_le_bytes()); // fWide
    for path in paths {
        for unit in wide(path).into_iter().chain([0]) {
            block.extend_from_slice(&unit.to_le_bytes());
        }
    }
    block.extend_from_slice(&[0, 0]);
    block
}

fn wide(path: &Path) -> Vec<u16> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        path.as_os_str().encode_wide().collect()
    }
    #[cfg(not(windows))]
    {
        path.to_string_lossy().encode_utf16().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(w: u32, h: u32) -> Rgba {
        let pixels = (0..w * h).flat_map(|i| [(i * 7) as u8, (i * 13) as u8, (i * 29) as u8, 255]).collect();
        Rgba { width: w, height: h, pixels }
    }

    #[test]
    fn png_roundtrip() {
        let image = sample(37, 21);
        let png = rgba_to_png(&image).unwrap();
        assert_eq!(&png[1..4], b"PNG");
        assert_eq!(png_to_rgba(&png).unwrap(), image);
    }

    #[test]
    fn dib_roundtrip_32_bit_bottom_up() {
        let image = sample(5, 3);
        assert_eq!(dib_to_rgba(&rgba_to_dib(&image)).unwrap(), image);
    }

    #[test]
    fn dib_24_bit_with_row_padding_and_top_down() {
        // 3×2 Pixel, 24 Bit, von oben nach unten: Zeilenlänge 9 Bytes → 12 mit Auffüllung
        let mut dib = rgba_to_dib(&sample(1, 1))[..40].to_vec();
        dib[4..8].copy_from_slice(&3i32.to_le_bytes());
        dib[8..12].copy_from_slice(&(-2i32).to_le_bytes());
        dib[14..16].copy_from_slice(&24u16.to_le_bytes());
        let rows = [[10u8, 20, 30, 40, 50, 60, 70, 80, 90, 0, 0, 0], [1, 2, 3, 4, 5, 6, 7, 8, 9, 0, 0, 0]];
        rows.iter().for_each(|r| dib.extend_from_slice(r));
        let image = dib_to_rgba(&dib).unwrap();
        assert_eq!((image.width, image.height), (3, 2));
        assert_eq!(&image.pixels[..4], &[30, 20, 10, 255], "BGR → RGB, erste Zeile oben");
        assert_eq!(&image.pixels[12..16], &[3, 2, 1, 255]);
    }

    #[test]
    fn dib_with_bitfields_and_zero_alpha() {
        // 2×1, 32 Bit, BI_BITFIELDS mit Masken hinter dem Kopf, Alpha überall 0
        let mut dib = rgba_to_dib(&sample(1, 1))[..40].to_vec();
        dib[4..8].copy_from_slice(&2i32.to_le_bytes());
        dib[8..12].copy_from_slice(&1i32.to_le_bytes());
        dib[16..20].copy_from_slice(&BI_BITFIELDS.to_le_bytes());
        for mask in [0x00FF_0000u32, 0x0000_FF00, 0x0000_00FF] {
            dib.extend_from_slice(&mask.to_le_bytes());
        }
        dib.extend_from_slice(&[0x11, 0x22, 0x33, 0x00, 0x44, 0x55, 0x66, 0x00]);
        let image = dib_to_rgba(&dib).unwrap();
        assert_eq!(image.pixels, vec![0x33, 0x22, 0x11, 255, 0x66, 0x55, 0x44, 255]);
    }

    #[test]
    fn unsupported_dibs_are_rejected() {
        let mut dib = rgba_to_dib(&sample(2, 2));
        dib[14..16].copy_from_slice(&8u16.to_le_bytes());
        assert!(dib_to_rgba(&dib).is_err());
        assert!(dib_to_rgba(&[0; 10]).is_err());
    }

    #[test]
    fn cf_html_roundtrip() {
        let fragment = "<b>Grüße</b> aus <i>Qross</i>";
        let block = build_cf_html(fragment);
        let text = String::from_utf8(block.clone()).unwrap();
        assert!(text.starts_with("Version:0.9\r\nStartHTML:"));
        assert_eq!(cf_html_fragment(&block).as_deref(), Some(fragment));
    }

    #[test]
    fn cf_html_from_browser_style_block() {
        // Wie es Browser ablegen: Offsets in Bytes, Ausschnitt mit Umlauten
        let body = "<html><body><!--StartFragment--><p>Hallo Wörld</p><!--EndFragment--></body></html>";
        let head = "Version:0.9\r\nStartHTML:0000000097\r\nEndHTML:0000000000\r\nStartFragment:0000000000\r\nEndFragment:0000000000\r\n";
        let start = head.len() + body.find("<p>").unwrap();
        let end = head.len() + body.find("<!--EndFragment-->").unwrap();
        let head = head
            .replace("StartFragment:0000000000", &format!("StartFragment:{start:010}"))
            .replace("EndFragment:0000000000", &format!("EndFragment:{end:010}"));
        let block = format!("{head}{body}");
        assert_eq!(cf_html_fragment(block.as_bytes()).as_deref(), Some("<p>Hallo Wörld</p>"));
        // Ohne Offsets: über die Kommentar-Marken
        assert_eq!(cf_html_fragment(body.as_bytes()).as_deref(), Some("<p>Hallo Wörld</p>"));
    }

    #[test]
    fn dropfiles_roundtrip_with_umlauts() {
        let paths = vec![PathBuf::from(r"C:\Users\Dennis\Bericht Grüße.pdf"), PathBuf::from(r"D:\Ordner")];
        let block = build_dropfiles(&paths);
        assert_eq!(&block[..4], &20u32.to_le_bytes());
        assert_eq!(&block[block.len() - 4..], &[0, 0, 0, 0], "doppelt nullterminiert");
        assert_eq!(parse_dropfiles(&block), paths);
    }

    #[test]
    fn dropfiles_with_8_bit_names_and_broken_blocks() {
        let mut block = vec![0u8; 20];
        block[0] = 20;
        block.extend_from_slice(b"C:\\alt.txt\0C:\\b\0\0");
        assert_eq!(parse_dropfiles(&block), vec![PathBuf::from(r"C:\alt.txt"), PathBuf::from(r"C:\b")]);
        assert!(parse_dropfiles(&[1, 2, 3]).is_empty());
        let mut far = build_dropfiles(&[PathBuf::from("x")]);
        far[0] = 200;
        assert!(parse_dropfiles(&far).is_empty(), "Versatz hinter dem Ende");
        assert!(parse_dropfiles(&build_dropfiles(&[])).is_empty());
    }
}
