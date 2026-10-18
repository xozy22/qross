//! Hilfen für unidirektionale Streams (erstes Byte = `StreamKind`).

use anyhow::{Context, Result};
use qross_proto::{write_frame, Clipboard, ClipboardData, DragMessage, StreamKind, MAX_FRAME_LEN};
use quinn::{Connection, RecvStream, SendStream};

/// Eingaben vor allem anderen senden; Clipboard nur, wenn die Leitung frei ist.
const INPUT_PRIORITY: i32 = 10;
pub const BULK_PRIORITY: i32 = -10;

pub async fn read_kind(stream: &mut RecvStream) -> Result<StreamKind> {
    let mut kind = [0u8; 1];
    stream.read_exact(&mut kind).await?;
    StreamKind::from_u8(kind[0]).with_context(|| format!("unbekannter Stream-Typ {}", kind[0]))
}

pub async fn open_input(conn: &Connection) -> Result<SendStream> {
    let mut stream = conn.open_uni().await?;
    stream.set_priority(INPUT_PRIORITY)?;
    stream.write_all(&[StreamKind::Input as u8]).await?;
    Ok(stream)
}

pub async fn send_clipboard(conn: Connection, mut data: ClipboardData) {
    // Ein Bild über der Frame-Grenze nicht mitschicken, Text und HTML aber schon
    if data.png.as_ref().is_some_and(|png| png.len() > MAX_FRAME_LEN - 1024 * 1024) {
        tracing::warn!("Bild in der Zwischenablage ist zu groß für die Übertragung – nur Text/HTML");
        data.png = None;
        if data.is_empty() {
            return;
        }
    }
    if let Err(e) = send_clipboard_message(&conn, &Clipboard::Data(data)).await {
        tracing::warn!("Clipboard konnte nicht gesendet werden: {e:#}");
    }
}

/// Eine Drag-Nachricht auf einem eigenen Stream – mit Vorrang, der Nutzer wartet an der Kante.
pub async fn send_drag_message(conn: &Connection, message: &DragMessage) -> Result<()> {
    let mut stream = conn.open_uni().await?;
    stream.set_priority(INPUT_PRIORITY)?;
    stream.write_all(&[StreamKind::Drag as u8]).await?;
    write_frame(&mut stream, message).await?;
    stream.finish()?;
    Ok(())
}

/// Eine Nachricht auf einem eigenen Clipboard-Stream (niedrige Priorität).
pub async fn send_clipboard_message(conn: &Connection, message: &Clipboard) -> Result<()> {
    let mut stream = conn.open_uni().await?;
    stream.set_priority(BULK_PRIORITY)?;
    stream.write_all(&[StreamKind::Clipboard as u8]).await?;
    write_frame(&mut stream, message).await?;
    stream.finish()?;
    Ok(())
}
