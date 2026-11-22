use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::Path;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path as AxumPath, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use axum_server::tls_rustls::RustlsConfig;
use futures_util::{SinkExt, StreamExt};
use karaok_audio::{AtomicEngineState, MicAudioProducer, MASTER_SAMPLE_RATE};
use karaok_core::{MicChannelId, MicPairingInfo};
use karaok_dsp::AdaptiveJitterBuffer;
use mdns_sd::{ServiceDaemon, ServiceInfo};
use serde::Deserialize;
use thiserror::Error;
use tokio::sync::{mpsc, Mutex};
use tracing::{info, warn};
use uuid::Uuid;

use crate::certs::{ensure_local_tls_certs, CertError};
use crate::mobile_ui::{render_mobile_mic_html, AUDIO_WORKLET_JS, CERT_INSTALL_HTML};
use crate::protocol::{AudioFramePacket, ClientControlMessage, ServerControlMessage};
use crate::qr::generate_qr_svg;

pub const DEFAULT_MIC_SERVER_PORT: u16 = 8443;
pub const MDNS_HOSTNAME: &str = "karaok-party.local";

#[derive(Debug, Error)]
pub enum MicServerError {
    #[error("TLS certificate error: {0}")]
    Cert(#[from] CertError),

    #[error("Server I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// Queue-addition request sent remotely from an iPhone microphone browser UI.
#[derive(Debug, Clone)]
pub struct RemoteQueueRequest {
    pub mic: MicChannelId,
    pub query: String,
    pub singer_name: String,
}

#[derive(Clone)]
struct ServerState {
    session_token: String,
    ca_cert_pem: String,
    engine_state: Arc<AtomicEngineState>,
    mic_producers: [Arc<Mutex<MicAudioProducer>>; 2],
    queue_tx: mpsc::UnboundedSender<RemoteQueueRequest>,
}

#[derive(Deserialize)]
struct AuthQuery {
    token: Option<String>,
}

/// Handle to the running local HTTPS/WSS microphone server + mDNS Bonjour advertiser.
pub struct MicServer {
    pairing_info: MicPairingInfo,
    _mdns_daemon: Option<ServiceDaemon>,
    handle: axum_server::Handle<SocketAddr>,
}

impl MicServer {
    /// Starts the local HTTPS/WSS server on `0.0.0.0:port`, advertises `karaok-party.local`
    /// via Bonjour/mDNS, and bridges Mic 1 & Mic 2 WebSocket frames into the CoreAudio ringbufs.
    pub async fn start(
        certs_dir: &Path,
        port: u16,
        engine_state: Arc<AtomicEngineState>,
        mic_producers: [MicAudioProducer; 2],
    ) -> Result<(Self, mpsc::UnboundedReceiver<RemoteQueueRequest>), MicServerError> {
        let _ = rustls::crypto::ring::default_provider().install_default();

        let lan_ip = local_ip_address::local_ip()
            .unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST));

        let tls_bundle = ensure_local_tls_certs(certs_dir, lan_ip)?;
        let rustls_config = RustlsConfig::from_pem(
            tls_bundle.server_cert_pem.into_bytes(),
            tls_bundle.server_key_pem.into_bytes(),
        )
        .await?;

        let session_token = Uuid::new_v4().simple().to_string()[..16].to_string();
        let (queue_tx, queue_rx) = mpsc::unbounded_channel();

        let [p1, p2] = mic_producers;
        let state = ServerState {
            session_token: session_token.clone(),
            ca_cert_pem: tls_bundle.ca_cert_pem,
            engine_state,
            mic_producers: [Arc::new(Mutex::new(p1)), Arc::new(Mutex::new(p2))],
            queue_tx,
        };

        let app = Router::new()
            .route("/mic/{id}", get(handle_mic_page))
            .route("/mic-worklet.js", get(handle_worklet_js))
            .route("/cert/install", get(handle_cert_install_page))
            .route("/cert/karaok-party-ca.pem", get(handle_ca_cert_download))
            .route("/ws/mic/{id}", get(handle_mic_ws))
            .with_state(state);

        let handle = axum_server::Handle::new();
        let server_handle = handle.clone();
        let addr = SocketAddr::from(([0, 0, 0, 0], port));

        tokio::spawn(async move {
            info!("Karaok Party HTTPS/WSS Mic Server listening on https://{addr}");
            if let Err(err) = axum_server::bind_rustls(addr, rustls_config)
                .handle(server_handle)
                .serve(app.into_make_service())
                .await
            {
                warn!("Mic HTTPS server exited: {err}");
            }
        });

        let mdns_daemon = register_mdns_service(lan_ip, port);

        // Prefer LAN IP in QR URLs for 100% reliability on iOS Wi-Fi routers that block mDNS,
        // while also advertising karaok-party.local via Bonjour.
        let host_for_qr = if lan_ip.is_loopback() {
            MDNS_HOSTNAME.to_string()
        } else {
            lan_ip.to_string()
        };
        let server_url = format!("https://{host_for_qr}:{port}");
        let ca_cert_url = format!("{server_url}/cert/install");
        let mic1_url = format!("{server_url}/mic/1?token={session_token}");
        let mic2_url = format!("{server_url}/mic/2?token={session_token}");

        let pairing_info = MicPairingInfo {
            server_url,
            mdns_hostname: format!("https://{MDNS_HOSTNAME}:{port}"),
            lan_ip: lan_ip.to_string(),
            port,
            ca_cert_url,
            mic1_qr_svg: generate_qr_svg(&mic1_url, "#0284c7"),
            mic2_qr_svg: generate_qr_svg(&mic2_url, "#e11d48"),
            mic1_url,
            mic2_url,
            session_token,
        };

        Ok((
            Self {
                pairing_info,
                _mdns_daemon: mdns_daemon,
                handle,
            },
            queue_rx,
        ))
    }

    pub fn pairing_info(&self) -> &MicPairingInfo {
        &self.pairing_info
    }

    pub fn shutdown(&self) {
        self.handle.shutdown();
    }
}

fn register_mdns_service(lan_ip: IpAddr, port: u16) -> Option<ServiceDaemon> {
    let daemon = ServiceDaemon::new().ok()?;
    let service_type = "_https._tcp.local.";
    let host_fqdn = format!("{MDNS_HOSTNAME}.");
    let info = ServiceInfo::new(
        service_type,
        "Karaok Party Studio",
        &host_fqdn,
        lan_ip,
        port,
        None,
    )
    .ok()?;
    if let Err(err) = daemon.register(info) {
        warn!("mDNS registration skipped: {err}");
        return None;
    }
    info!("Advertised Bonjour mDNS service https://{MDNS_HOSTNAME}:{port}");
    Some(daemon)
}

async fn handle_mic_page(
    AxumPath(id): AxumPath<u8>,
    Query(q): Query<AuthQuery>,
    State(state): State<ServerState>,
) -> Response {
    if MicChannelId::from_one_based(id).is_none() {
        return (StatusCode::NOT_FOUND, "Invalid mic channel (use 1 or 2)").into_response();
    }
    if q.token.as_deref() != Some(state.session_token.as_str()) {
        return (
            StatusCode::UNAUTHORIZED,
            "Jeton d'appairage manquant ou invalide. Scannez le QR Code affiché sur le Mac.",
        )
            .into_response();
    }
    Html(render_mobile_mic_html(id, &state.session_token)).into_response()
}

async fn handle_worklet_js() -> Response {
    (
        [(
            header::CONTENT_TYPE,
            "application/javascript; charset=utf-8",
        )],
        AUDIO_WORKLET_JS,
    )
        .into_response()
}

async fn handle_cert_install_page() -> Response {
    Html(CERT_INSTALL_HTML).into_response()
}

async fn handle_ca_cert_download(State(state): State<ServerState>) -> Response {
    (
        [
            (header::CONTENT_TYPE, "application/x-pem-file"),
            (
                header::CONTENT_DISPOSITION,
                "attachment; filename=\"karaok-party-ca.pem\"",
            ),
        ],
        state.ca_cert_pem,
    )
        .into_response()
}

async fn handle_mic_ws(
    AxumPath(id): AxumPath<u8>,
    Query(q): Query<AuthQuery>,
    ws: WebSocketUpgrade,
    State(state): State<ServerState>,
) -> Response {
    let Some(channel) = MicChannelId::from_one_based(id) else {
        return (StatusCode::NOT_FOUND, "Unknown mic channel").into_response();
    };
    if q.token.as_deref() != Some(state.session_token.as_str()) {
        return (StatusCode::UNAUTHORIZED, "Invalid pairing session token").into_response();
    }
    ws.on_upgrade(move |socket| serve_mic_websocket(socket, channel, state))
}

async fn serve_mic_websocket(socket: WebSocket, channel: MicChannelId, state: ServerState) {
    let (mut ws_tx, mut ws_rx) = socket.split();
    let idx = channel.index();
    let mut jitter_buf = AdaptiveJitterBuffer::new(MASTER_SAMPLE_RATE as f32);
    let mut drain_scratch = vec![0.0_f32; 512];
    let mut last_telemetry_push = Instant::now();

    state
        .engine_state
        .mic_telemetry(channel)
        .store_network(true, 12.0, 0.0);
    info!("iPhone connected on Mic {}", channel.one_based());

    while let Some(Ok(msg)) = ws_rx.next().await {
        match msg {
            Message::Binary(bytes) => {
                if let Some(pkt) = AudioFramePacket::decode(&bytes) {
                    let now_us = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map(|d| d.as_micros() as u64)
                        .unwrap_or(0);
                    let count = pkt.samples.len();
                    jitter_buf.push_packet(pkt.seq, now_us, &pkt.samples);

                    // Drain `count` smoothed samples from AdaptiveJitterBuffer into CoreAudio lock-free SPSC
                    let to_pop = count.min(drain_scratch.len());
                    for slot in drain_scratch.iter_mut().take(to_pop) {
                        *slot = jitter_buf.pop_sample();
                    }
                    {
                        let mut prod = state.mic_producers[idx].lock().await;
                        prod.push_samples(&drain_scratch[..to_pop]);
                    }

                    let stats = jitter_buf.stats();
                    state.engine_state.mic_telemetry(channel).store_network(
                        true,
                        stats.target_delay_ms,
                        stats.packet_loss_pct,
                    );

                    // Push telemetry back to iPhone at ~12 Hz
                    if last_telemetry_push.elapsed().as_millis() >= 80 {
                        last_telemetry_push = Instant::now();
                        let t = state.engine_state.mic_telemetry(channel).load();
                        let payload = ServerControlMessage::Telemetry {
                            vu_db: t.vu_db,
                            peak: t.peak,
                            jitter_ms: t.jitter_ms,
                            connected: true,
                            muted: t.muted,
                        };
                        if let Ok(json) = serde_json::to_string(&payload) {
                            let _ = ws_tx.send(Message::Text(json.into())).await;
                        }
                    }
                }
            }
            Message::Text(text) => {
                if let Ok(ctrl) = serde_json::from_str::<ClientControlMessage>(&text) {
                    match ctrl {
                        ClientControlMessage::Mute { muted } => {
                            state.engine_state.mic_params(channel).set_muted(muted);
                        }
                        ClientControlMessage::Gain { value } => {
                            state.engine_state.mic_params(channel).set_gain(value);
                        }
                        ClientControlMessage::QueueAdd { query, singer_name } => {
                            let singer = singer_name.unwrap_or_else(|| {
                                format!("Micro {}", channel.one_based())
                            });
                            let _ = state.queue_tx.send(RemoteQueueRequest {
                                mic: channel,
                                query: query.clone(),
                                singer_name: singer,
                            });
                            let ack = ServerControlMessage::QueueAck {
                                message: format!("Demandé : {query}"),
                            };
                            if let Ok(json) = serde_json::to_string(&ack) {
                                let _ = ws_tx.send(Message::Text(json.into())).await;
                            }
                        }
                        ClientControlMessage::Ping { t } => {
                            if let Ok(json) = serde_json::to_string(&ServerControlMessage::Pong { t })
                            {
                                let _ = ws_tx.send(Message::Text(json.into())).await;
                            }
                        }
                    }
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }

    state
        .engine_state
        .mic_telemetry(channel)
        .store_network(false, 0.0, 0.0);
    info!("iPhone disconnected from Mic {}", channel.one_based());
}

#[cfg(test)]
mod tests {
    use super::*;
    use karaok_audio::AudioEngine;

    #[tokio::test]
    async fn mic_page_requires_valid_session_token() {
        let (engine, [p1, p2]) = AudioEngine::new();
        let (tx, _rx) = mpsc::unbounded_channel();
        let state = ServerState {
            session_token: "secret-token-123".into(),
            ca_cert_pem: String::new(),
            engine_state: engine.state(),
            mic_producers: [Arc::new(Mutex::new(p1)), Arc::new(Mutex::new(p2))],
            queue_tx: tx,
        };

        let missing = handle_mic_page(
            AxumPath(1),
            Query(AuthQuery { token: None }),
            State(state.clone()),
        )
        .await;
        assert_eq!(missing.status(), StatusCode::UNAUTHORIZED);

        let wrong = handle_mic_page(
            AxumPath(1),
            Query(AuthQuery {
                token: Some("wrong-token".into()),
            }),
            State(state.clone()),
        )
        .await;
        assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED);

        let ok = handle_mic_page(
            AxumPath(1),
            Query(AuthQuery {
                token: Some("secret-token-123".into()),
            }),
            State(state),
        )
        .await;
        assert_eq!(ok.status(), StatusCode::OK);
    }
}
