//! Local HTTPS / WSS wireless iPhone microphone server for Karaok Party (Duo Mode).
//!
//! Enables 1 or 2 iPhones to pair instantly via QR Code in Safari iOS without installing
//! any native iOS app or holding an Apple Developer account:
//! - Self-hosted Local CA + TLS certificate (`rcgen` + `rustls`)
//! - Zeroconf / Bonjour `karaok-party.local` advertisement (`mdns-sd`)
//! - Ultra-low-latency 128-sample binary WebSocket frames from Safari `AudioWorklet`
//! - Adaptive Jitter Buffer (10–35 ms) bridging directly into CoreAudio's lock-free `ringbuf`

pub mod certs;
pub mod mobile_ui;
pub mod protocol;
pub mod qr;
pub mod server;

pub use certs::{ensure_local_tls_certs, LocalTlsBundle};
pub use protocol::{AudioFramePacket, ClientControlMessage, ServerControlMessage};
pub use qr::generate_qr_svg;
pub use server::{MicServer, MicServerError, RemoteQueueRequest};
