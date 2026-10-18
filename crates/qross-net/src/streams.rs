//! Hilfen für unidirektionale Streams (erstes Byte = `StreamKind`).

use anyhow::{Context, Result};
use qross_proto::{write_frame, Clipboard, StreamKind};
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

pub async fn send_clipboard(conn: Connection, text: String) {
    let result: Result<()> = async {
        let mut stream = conn.open_uni().await?;
        stream.set_priority(BULK_PRIORITY)?;
        stream.write_all(&[StreamKind::Clipboard as u8]).await?;
        write_frame(&mut stream, &Clipboard::Text(text)).await?;
        stream.finish()?;
        Ok(())
    }
    .await;
    if let Err(e) = result {
        tracing::warn!("Clipboard konnte nicht gesendet werden: {e:#}");
    }
}
