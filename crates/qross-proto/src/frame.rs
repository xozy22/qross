//! Framing: Auf Streams `u32`-Längenpräfix (big-endian) + postcard; Datagramme sind reines postcard.

use std::io;

use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::Datagram;

/// Obergrenze für einen Frame (schützt vor Speicherangriffen über das Längenfeld).
pub const MAX_FRAME_LEN: usize = 64 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("Ein-/Ausgabefehler: {0}")]
    Io(#[from] io::Error),
    #[error("ungültige Nachricht: {0}")]
    Codec(#[from] postcard::Error),
    #[error("Frame zu groß: {0} Bytes")]
    TooLarge(usize),
}

pub async fn write_frame<W, T>(w: &mut W, msg: &T) -> Result<(), FrameError>
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    let body = postcard::to_stdvec(msg)?;
    if body.len() > MAX_FRAME_LEN {
        return Err(FrameError::TooLarge(body.len()));
    }
    let mut buf = Vec::with_capacity(4 + body.len());
    buf.extend_from_slice(&(body.len() as u32).to_be_bytes());
    buf.extend_from_slice(&body);
    w.write_all(&buf).await?;
    w.flush().await?;
    Ok(())
}

/// Liest einen Frame. `Ok(None)` bei sauberem Stream-Ende vor einem neuen Frame.
pub async fn read_frame<R, T>(r: &mut R) -> Result<Option<T>, FrameError>
where
    R: AsyncRead + Unpin,
    T: DeserializeOwned,
{
    read_frame_up_to(r, MAX_FRAME_LEN).await
}

/// Wie [`read_frame`], aber nur bis `max` Bytes, etwa solange die Gegenstelle noch nicht geprüft ist:
/// Der Puffer wird nach dem Längenfeld angelegt, bevor ein Byte des Inhalts da ist.
pub async fn read_frame_up_to<R, T>(r: &mut R, max: usize) -> Result<Option<T>, FrameError>
where
    R: AsyncRead + Unpin,
    T: DeserializeOwned,
{
    let mut len = [0u8; 4];
    match r.read_exact(&mut len).await {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e.into()),
    }
    let len = u32::from_be_bytes(len) as usize;
    if len > max.min(MAX_FRAME_LEN) {
        return Err(FrameError::TooLarge(len));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).await?;
    Ok(Some(postcard::from_bytes(&buf)?))
}

pub fn encode_datagram(d: &Datagram) -> Vec<u8> {
    postcard::to_stdvec(d).expect("Datagramme sind immer serialisierbar")
}

pub fn decode_datagram(bytes: &[u8]) -> Result<Datagram, FrameError> {
    Ok(postcard::from_bytes(bytes)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Control, Input, KeyCode, MouseButton, MouseMove, Platform, Rect, ScreenInfo};

    #[tokio::test]
    async fn frames_roundtrip() {
        let (mut a, mut b) = tokio::io::duplex(1024);
        let hello = Control::Hello {
            version: 1,
            name: "laptop".into(),
            screen: ScreenInfo { bounds: Rect::new(-1920, 0, 3840, 1080) },
            platform: Platform::MacOs,
        };
        let inputs = [
            Input::Enter { epoch: 7, x: 1, y: 500 },
            Input::Key { key: KeyCode(0x04), down: true },
            Input::Button { button: MouseButton::Forward, down: false, x: -5, y: 9 },
            Input::Wheel { dx: 0, dy: -120 },
            Input::Leave,
        ];
        write_frame(&mut a, &hello).await.unwrap();
        for i in &inputs {
            write_frame(&mut a, i).await.unwrap();
        }
        drop(a);

        assert_eq!(read_frame::<_, Control>(&mut b).await.unwrap(), Some(hello));
        for i in &inputs {
            assert_eq!(read_frame::<_, Input>(&mut b).await.unwrap().as_ref(), Some(i));
        }
        assert_eq!(read_frame::<_, Input>(&mut b).await.unwrap(), None);
    }

    #[tokio::test]
    async fn oversized_length_is_rejected() {
        let (mut a, mut b) = tokio::io::duplex(64);
        a.write_all(&u32::MAX.to_be_bytes()).await.unwrap();
        assert!(matches!(read_frame::<_, Input>(&mut b).await, Err(FrameError::TooLarge(_))));
    }

    #[tokio::test]
    async fn a_smaller_limit_applies_before_any_content() {
        let (mut a, mut b) = tokio::io::duplex(64);
        // Nur das Längenfeld: Mehr braucht es nicht, um abgewiesen zu werden.
        a.write_all(&(16 * 1024 + 1u32).to_be_bytes()).await.unwrap();
        assert!(matches!(read_frame_up_to::<_, Control>(&mut b, 16 * 1024).await, Err(FrameError::TooLarge(16385))));

        let (mut a, mut b) = tokio::io::duplex(1024);
        let hello = Control::Hello {
            version: 3,
            name: "laptop".into(),
            screen: ScreenInfo { bounds: Rect::new(0, 0, 1512, 982) },
            platform: Platform::MacOs,
        };
        write_frame(&mut a, &hello).await.unwrap();
        assert_eq!(read_frame_up_to::<_, Control>(&mut b, 16 * 1024).await.unwrap(), Some(hello));
    }

    #[test]
    fn mouse_datagram_is_tiny() {
        let d = Datagram::MouseMove(MouseMove { epoch: 3, seq: 123_456, x: 2559, y: 1439 });
        let bytes = encode_datagram(&d);
        assert!(bytes.len() <= 12, "{} Bytes", bytes.len());
        assert_eq!(decode_datagram(&bytes).unwrap(), d);
    }
}
