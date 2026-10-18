//! Minimales Nachrichtenformat für den Benchmark.
//!
//! Nachricht: `kind u8 | seq u32 | ts u64 | payload`. Auf Streams kommt ein
//! `u32`-Längenpräfix davor (wie bei Synergy), Datagramme tragen die Nachricht direkt.

use std::io;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::workload::{Clock, Event, Recorder};

pub const HEADER_LEN: usize = 13;
/// Clipboard wird in Chunks dieser Größe übertragen.
pub const CLIP_CHUNK: usize = 64 * 1024;
/// Eine Mausbewegung trägt zwei 16-Bit-Koordinaten (wie beim quelloffenen Deskflow).
const INPUT_PAYLOAD: [u8; 4] = [0, 100, 0, 50];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Mouse = 1,
    Key = 2,
    Clip = 3,
}

pub struct Msg<'a> {
    pub kind: Kind,
    pub seq: u32,
    pub ts: u64,
    pub payload: &'a [u8],
}

pub fn encode(kind: Kind, seq: u32, ts: u64, payload: &[u8]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(HEADER_LEN + payload.len());
    buf.push(kind as u8);
    buf.extend_from_slice(&seq.to_be_bytes());
    buf.extend_from_slice(&ts.to_be_bytes());
    buf.extend_from_slice(payload);
    buf
}

pub fn encode_input(ev: &Event) -> Vec<u8> {
    encode(ev.kind, ev.seq, ev.ts, &INPUT_PAYLOAD)
}

fn framed(msg: Vec<u8>) -> Vec<u8> {
    let mut buf = Vec::with_capacity(4 + msg.len());
    buf.extend_from_slice(&(msg.len() as u32).to_be_bytes());
    buf.extend_from_slice(&msg);
    buf
}

pub fn decode(buf: &[u8]) -> Option<Msg<'_>> {
    if buf.len() < HEADER_LEN {
        return None;
    }
    let kind = match buf[0] {
        1 => Kind::Mouse,
        2 => Kind::Key,
        3 => Kind::Clip,
        _ => return None,
    };
    Some(Msg {
        kind,
        seq: u32::from_be_bytes(buf[1..5].try_into().unwrap()),
        ts: u64::from_be_bytes(buf[5..13].try_into().unwrap()),
        payload: &buf[HEADER_LEN..],
    })
}

/// Schreibt ein Ereignis auf einen Stream; Clipboard-Daten werden in Chunks zerlegt.
pub async fn write_event<W: AsyncWrite + Unpin>(w: &mut W, ev: &Event) -> io::Result<()> {
    match ev.kind {
        Kind::Mouse | Kind::Key => w.write_all(&framed(encode_input(ev))).await?,
        Kind::Clip => {
            let mut remaining = ev.clip_len;
            while remaining > 0 {
                let n = remaining.min(CLIP_CHUNK);
                remaining -= n;
                let mut payload = vec![0u8; 1 + n];
                payload[0] = (remaining == 0) as u8;
                w.write_all(&framed(encode(Kind::Clip, ev.seq, ev.ts, &payload))).await?;
            }
        }
    }
    w.flush().await
}

/// Liest längengerahmte Nachrichten, bis der Stream endet.
pub async fn read_frames<R: AsyncRead + Unpin>(r: &mut R, clock: Clock, rec: &Recorder) -> io::Result<()> {
    let mut buf = Vec::new();
    loop {
        let mut len = [0u8; 4];
        match r.read_exact(&mut len).await {
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(e) => return Err(e),
        }
        buf.resize(u32::from_be_bytes(len) as usize, 0);
        r.read_exact(&mut buf).await?;
        let now = clock.now_ns();
        if let Some(msg) = decode(&buf) {
            rec.on_message(&msg, now);
        }
    }
}
