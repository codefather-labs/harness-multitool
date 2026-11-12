//! A WebSocket for the dashboard (CON-152 as journal#1039 has it, decision journal#1040): the
//! daemon pushes, the page listens. RFC 6455, only what a one-way feed needs: the handshake, text
//! frames out, and from the client close and ping (answered by pong); anything else it sends is
//! read and ignored.

use std::io::{Read, Write};

use base64::Engine;
use sha1::{Digest, Sha1};

/// The key's answer in the handshake: base64 of SHA-1 of the key and the RFC's GUID.
pub fn accept(key: &str) -> String {
    let mut hash = Sha1::new();
    hash.update(key.trim().as_bytes());
    hash.update(b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
    base64::engine::general_purpose::STANDARD.encode(hash.finalize())
}

/// A frame from the server: final, unmasked.
pub fn frame(opcode: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = vec![0x80 | opcode];
    match payload.len() {
        n if n < 126 => out.push(n as u8),
        n if n <= u16::MAX as usize => {
            out.push(126);
            out.extend((n as u16).to_be_bytes());
        }
        n => {
            out.push(127);
            out.extend((n as u64).to_be_bytes());
        }
    }
    out.extend_from_slice(payload);
    out
}

pub fn send_text(stream: &mut dyn Write, text: &str) -> std::io::Result<()> {
    stream.write_all(&frame(0x1, text.as_bytes()))?;
    stream.flush()
}

/// What the client sent: a close, a ping with its payload, or something else.
#[derive(Debug, PartialEq)]
pub enum Incoming {
    Close,
    Ping(Vec<u8>),
    Other,
}

/// Reads one frame from the client, unmasking it.
pub fn read_frame(stream: &mut dyn Read) -> std::io::Result<Incoming> {
    let mut head = [0u8; 2];
    stream.read_exact(&mut head)?;
    let opcode = head[0] & 0x0f;
    let masked = head[1] & 0x80 != 0;
    let mut len = u64::from(head[1] & 0x7f);
    if len == 126 {
        let mut b = [0u8; 2];
        stream.read_exact(&mut b)?;
        len = u64::from(u16::from_be_bytes(b));
    } else if len == 127 {
        let mut b = [0u8; 8];
        stream.read_exact(&mut b)?;
        len = u64::from_be_bytes(b);
    }
    // A dashboard sends next to nothing; a huge frame is no client of ours.
    if len > 1 << 20 {
        return Ok(Incoming::Close);
    }
    let mut mask = [0u8; 4];
    if masked {
        stream.read_exact(&mut mask)?;
    }
    let mut payload = vec![0u8; len as usize];
    stream.read_exact(&mut payload)?;
    if masked {
        for (i, b) in payload.iter_mut().enumerate() {
            *b ^= mask[i % 4];
        }
    }
    Ok(match opcode {
        0x8 => Incoming::Close,
        0x9 => Incoming::Ping(payload),
        _ => Incoming::Other,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 6455 section 1.3's own example.
    #[test]
    fn the_handshake_answers_as_the_rfc_says() {
        assert_eq!(accept("dGhlIHNhbXBsZSBub25jZQ=="), "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
    }

    #[test]
    fn frames_go_out_and_come_in() {
        assert_eq!(frame(0x1, b"hi"), [0x81, 2, b'h', b'i']);
        let long = frame(0x1, &[b'x'; 300]);
        assert_eq!((long[1], u16::from_be_bytes([long[2], long[3]])), (126, 300));
        // A masked ping from a client, and a masked close.
        let mask = [1u8, 2, 3, 4];
        let ping: Vec<u8> = [0x89, 0x80 | 3].into_iter().chain(mask).chain(b"abc".iter().enumerate().map(|(i, b)| b ^ mask[i % 4])).collect();
        assert_eq!(read_frame(&mut &ping[..]).unwrap(), Incoming::Ping(b"abc".to_vec()));
        let close: Vec<u8> = [0x88, 0x80].into_iter().chain(mask).collect();
        assert_eq!(read_frame(&mut &close[..]).unwrap(), Incoming::Close);
    }
}
