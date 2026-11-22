use serde::{Deserialize, Serialize};

/// Magic header byte (`'K'`) for Karaok Party binary WebSocket audio frames.
pub const PACKET_MAGIC: u8 = 0x4B;
pub const HEADER_LEN: usize = 16;
pub const FORMAT_INT16: u8 = 1;
pub const FORMAT_FLOAT32: u8 = 2;
pub const MAX_SAMPLES_PER_PACKET: usize = 1024;

/// Decoded binary audio frame transmitted by Safari iOS `AudioWorklet` (~128 samples / 2.67 ms).
#[derive(Debug, Clone)]
pub struct AudioFramePacket {
    pub seq: u32,
    pub timestamp_us: u64,
    pub samples: Vec<f32>,
}

impl AudioFramePacket {
    /// Parses a binary WebSocket message into normalized `f32` PCM samples (`-1.0 .. 1.0`).
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < HEADER_LEN || bytes[0] != PACKET_MAGIC {
            return None;
        }
        let format = bytes[1];
        let sample_count = u16::from_le_bytes([bytes[2], bytes[3]]) as usize;
        if sample_count == 0 || sample_count > MAX_SAMPLES_PER_PACKET {
            return None;
        }
        let seq = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
        let timestamp_us = u64::from_le_bytes([
            bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15],
        ]);
        let payload = &bytes[HEADER_LEN..];

        let mut samples = Vec::with_capacity(sample_count);
        match format {
            FORMAT_INT16 => {
                if payload.len() < sample_count * 2 {
                    return None;
                }
                for chunk in payload[..sample_count * 2].chunks_exact(2) {
                    let s = i16::from_le_bytes([chunk[0], chunk[1]]);
                    samples.push(s as f32 / 32768.0);
                }
            }
            FORMAT_FLOAT32 => {
                if payload.len() < sample_count * 4 {
                    return None;
                }
                for chunk in payload[..sample_count * 4].chunks_exact(4) {
                    let f = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                    samples.push(if f.is_finite() { f.clamp(-1.0, 1.0) } else { 0.0 });
                }
            }
            _ => return None,
        }

        Some(Self {
            seq,
            timestamp_us,
            samples,
        })
    }

    /// Encodes a packet into little-endian Int16 PCM wire format.
    pub fn encode_int16(&self) -> Vec<u8> {
        let count = self.samples.len().min(MAX_SAMPLES_PER_PACKET) as u16;
        let mut buf = Vec::with_capacity(HEADER_LEN + (count as usize) * 2);
        buf.push(PACKET_MAGIC);
        buf.push(FORMAT_INT16);
        buf.extend_from_slice(&count.to_le_bytes());
        buf.extend_from_slice(&self.seq.to_le_bytes());
        buf.extend_from_slice(&self.timestamp_us.to_le_bytes());
        for &s in self.samples.iter().take(count as usize) {
            let i = (s.clamp(-1.0, 1.0) * 32767.0).round() as i16;
            buf.extend_from_slice(&i.to_le_bytes());
        }
        buf
    }
}

/// JSON remote control commands sent from the iPhone web UI to the Mac.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientControlMessage {
    Mute { muted: bool },
    Gain { value: f32 },
    QueueAdd { query: String, singer_name: Option<String> },
    Ping { t: u64 },
}

/// JSON real-time telemetry pushed from the Mac back to the iPhone UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerControlMessage {
    Telemetry {
        vu_db: f32,
        peak: f32,
        jitter_ms: f32,
        connected: bool,
        muted: bool,
    },
    Pong {
        t: u64,
    },
    QueueAck {
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binary_packet_int16_roundtrip() {
        let pkt = AudioFramePacket {
            seq: 42,
            timestamp_us: 123_456_789,
            samples: vec![0.0, 0.5, -0.5, 0.99],
        };
        let encoded = pkt.encode_int16();
        assert_eq!(encoded.len(), HEADER_LEN + 8);
        let decoded = AudioFramePacket::decode(&encoded).unwrap();
        assert_eq!(decoded.seq, 42);
        assert_eq!(decoded.timestamp_us, 123_456_789);
        assert_eq!(decoded.samples.len(), 4);
        assert!((decoded.samples[1] - 0.5).abs() < 0.001);
        assert!((decoded.samples[2] + 0.5).abs() < 0.001);
    }

    #[test]
    fn rejects_invalid_magic_or_truncated_payload() {
        assert!(AudioFramePacket::decode(&[0x00; 20]).is_none());
        assert!(AudioFramePacket::decode(&[PACKET_MAGIC, FORMAT_INT16, 128, 0]).is_none());
    }
}
