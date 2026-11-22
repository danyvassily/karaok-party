//! Embedded OLED dark-mode Safari iOS mobile microphone web app, `AudioWorkletProcessor`,
//! and 1-tap Local CA installation assistant.

/// JavaScript `AudioWorkletProcessor` slicing incoming 48 kHz mono audio into ultra-short
/// 128-sample frames (~2.67 ms) encoded as Int16 PCM with the 16-byte Karaok Party binary header.
pub const AUDIO_WORKLET_JS: &str = r#"
class KaraokMicProcessor extends AudioWorkletProcessor {
  constructor() {
    super();
    this.seq = 0;
  }

  process(inputs) {
    const input = inputs[0];
    if (!input || !input[0]) return true;
    const channel = input[0];
    const len = channel.length; // 128 samples standard Web Audio quantum
    const buffer = new ArrayBuffer(16 + len * 2);
    const view = new DataView(buffer);

    // Header: Magic 'K' (0x4B), Format 1 (Int16), count (u16), seq (u32), timestamp_us (u64)
    view.setUint8(0, 0x4b);
    view.setUint8(1, 1);
    view.setUint16(2, len, true);
    view.setUint32(4, this.seq >>> 0, true);
    const tsUs = BigInt(Math.round(currentTime * 1_000_000));
    view.setBigUint64(8, tsUs, true);

    let peak = 0;
    for (let i = 0; i < len; i++) {
      const s = Math.max(-1, Math.min(1, channel[i]));
      const abs = Math.abs(s);
      if (abs > peak) peak = abs;
      view.setInt16(16 + i * 2, Math.round(s * 32767), true);
    }

    this.seq = (this.seq + 1) >>> 0;
    this.port.postMessage({ buffer, peak }, [buffer]);
    return true;
  }
}

registerProcessor('karaok-mic-processor', KaraokMicProcessor);
"#;

/// Renders the Safari iOS mobile microphone page for Mic 1 (Blue) or Mic 2 (Coral).
pub fn render_mobile_mic_html(mic_id: u8, session_token: &str) -> String {
    let (label, accent, accent_glow) = if mic_id == 1 {
        ("Micro 1", "#38bdf8", "rgba(56, 189, 248, 0.35)")
    } else {
        ("Micro 2", "#fb7185", "rgba(251, 113, 133, 0.35)")
    };

    format!(
        r#"<!DOCTYPE html>
<html lang="fr">
<head>
<meta charset="utf-8" />
<meta name="viewport" content="width=device-width, initial-scale=1, maximum-scale=1, user-scalable=no, viewport-fit=cover" />
<meta name="apple-mobile-web-app-capable" content="yes" />
<meta name="apple-mobile-web-app-status-bar-style" content="black-translucent" />
<title>Karaok Party — {label}</title>
<style>
  :root {{
    --bg: #000000;
    --surface: #0e1017;
    --border: rgba(255,255,255,0.10);
    --accent: {accent};
    --glow: {accent_glow};
    --text: #f8fafc;
    --muted: #94a3b8;
  }}
  * {{ box-sizing: border-box; -webkit-tap-highlight-color: transparent; }}
  body {{
    margin: 0;
    min-height: 100dvh;
    background: var(--bg);
    color: var(--text);
    font-family: -apple-system, BlinkMacSystemFont, "SF Pro Display", sans-serif;
    display: flex;
    flex-direction: column;
    padding: max(18px, env(safe-area-inset-top)) 20px max(22px, env(safe-area-inset-bottom));
    gap: 18px;
  }}
  header {{
    display: flex;
    align-items: center;
    justify-content: space-between;
  }}
  .badge {{
    display: inline-flex;
    align-items: center;
    gap: 8px;
    padding: 6px 12px;
    border-radius: 999px;
    background: var(--surface);
    border: 1px solid var(--border);
    font-size: 13px;
    font-weight: 600;
  }}
  .dot {{
    width: 9px; height: 9px; border-radius: 50%;
    background: #64748b;
  }}
  .dot.live {{ background: #22c55e; box-shadow: 0 0 10px #22c55e; }}
  .stage {{
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 20px;
  }}
  .vu-wrap {{
    position: relative;
    width: 224px;
    height: 224px;
    display: grid;
    place-items: center;
  }}
  .vu-ring {{
    position: absolute;
    inset: 0;
    border-radius: 50%;
    border: 4px solid rgba(255,255,255,0.08);
    box-shadow: 0 0 0 0 var(--glow);
    transition: transform 45ms linear, box-shadow 45ms linear;
  }}
  .mic-btn {{
    width: 172px;
    height: 172px;
    border-radius: 50%;
    border: 2px solid var(--accent);
    background: radial-gradient(circle at 30% 30%, #181c28, #090b10);
    color: var(--text);
    font-size: 18px;
    font-weight: 700;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 8px;
    cursor: pointer;
    z-index: 2;
  }}
  .mic-btn.muted {{
    border-color: #ef4444;
    color: #fca5a5;
  }}
  .card {{
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 16px;
    padding: 14px 16px;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }}
  .row {{ display: flex; justify-content: space-between; align-items: center; font-size: 14px; }}
  input[type=range] {{
    width: 100%;
    accent-color: var(--accent);
    height: 28px;
  }}
  .queue-form {{ display: flex; gap: 8px; }}
  .queue-form input {{
    flex: 1;
    background: #05060a;
    border: 1px solid var(--border);
    border-radius: 10px;
    padding: 10px 12px;
    color: var(--text);
    font-size: 15px;
  }}
  .queue-form button {{
    background: var(--accent);
    color: #020617;
    border: none;
    border-radius: 10px;
    padding: 10px 14px;
    font-weight: 700;
    font-size: 14px;
  }}
  .meta {{ color: var(--muted); font-size: 12px; text-align: center; }}
</style>
</head>
<body>
  <header>
    <div class="badge"><span class="dot" id="statusDot"></span><span id="statusText">Prêt</span></div>
    <div class="badge" style="color: var(--accent)">{label}</div>
  </header>

  <main class="stage">
    <div class="vu-wrap">
      <div class="vu-ring" id="vuRing"></div>
      <button class="mic-btn" id="mainBtn" type="button">
        <span id="btnIcon" style="font-size: 38px;">🎙️</span>
        <span id="btnLabel">Activer le micro</span>
      </button>
    </div>
    <div class="meta" id="latencyMeta">48 kHz · 128 éch. (2,67 ms) · Veille désactivée</div>
  </main>

  <section class="card">
    <div class="row">
      <span>Volume vocal</span>
      <strong id="gainVal">100 %</strong>
    </div>
    <input id="gainSlider" type="range" min="0" max="180" value="100" />
  </section>

  <section class="card">
    <div class="row">
      <span>Télécommande file d'attente</span>
      <span id="queueFeedback" style="color: var(--accent); font-size: 12px;"></span>
    </div>
    <form class="queue-form" id="queueForm">
      <input id="queueInput" type="text" placeholder="Ajouter un titre ou artiste..." autocomplete="off" />
      <button type="submit">+ File</button>
    </form>
  </section>

<script>
(() => {{
  const MIC_ID = {mic_id};
  const TOKEN = {token_json};
  const mainBtn = document.getElementById('mainBtn');
  const btnLabel = document.getElementById('btnLabel');
  const btnIcon = document.getElementById('btnIcon');
  const statusDot = document.getElementById('statusDot');
  const statusText = document.getElementById('statusText');
  const vuRing = document.getElementById('vuRing');
  const latencyMeta = document.getElementById('latencyMeta');
  const gainSlider = document.getElementById('gainSlider');
  const gainVal = document.getElementById('gainVal');
  const queueForm = document.getElementById('queueForm');
  const queueInput = document.getElementById('queueInput');
  const queueFeedback = document.getElementById('queueFeedback');

  let ws = null;
  let audioCtx = null;
  let started = false;
  let muted = false;
  let wakeLock = null;

  async function requestWakeLock() {{
    try {{
      if ('wakeLock' in navigator) {{
        wakeLock = await navigator.wakeLock.request('screen');
      }}
    }} catch (_) {{}}
  }}

  async function startMic() {{
    try {{
      statusText.textContent = 'Connexion...';
      await requestWakeLock();

      const stream = await navigator.mediaDevices.getUserMedia({{
        audio: {{
          echoCancellation: false,
          noiseSuppression: false,
          autoGainControl: false,
          sampleRate: 48000,
          channelCount: 1
        }}
      }});

      audioCtx = new (window.AudioContext || window.webkitAudioContext)({{
        sampleRate: 48000,
        latencyHint: 'interactive'
      }});
      await audioCtx.audioWorklet.addModule('/mic-worklet.js');

      const source = audioCtx.createMediaStreamSource(stream);
      const worklet = new AudioWorkletNode(audioCtx, 'karaok-mic-processor');
      source.connect(worklet);

      const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
      ws = new WebSocket(`${{proto}}//${{location.host}}/ws/mic/${{MIC_ID}}?token=${{encodeURIComponent(TOKEN)}}`);
      ws.binaryType = 'arraybuffer';

      ws.onopen = () => {{
        started = true;
        statusDot.classList.add('live');
        statusText.textContent = 'En direct';
        btnLabel.textContent = 'Couper (Mute)';
      }};

      ws.onmessage = (ev) => {{
        try {{
          const msg = JSON.parse(ev.data);
          if (msg.type === 'telemetry') {{
            latencyMeta.textContent = `48 kHz · Jitter ${{msg.jitter_ms.toFixed(1)}} ms · ${{msg.vu_db.toFixed(0)}} dB`;
          }} else if (msg.type === 'queue_ack') {{
            queueFeedback.textContent = msg.message;
            setTimeout(() => queueFeedback.textContent = '', 3000);
          }}
        }} catch (_) {{}}
      }};

      ws.onclose = () => {{
        statusDot.classList.remove('live');
        statusText.textContent = 'Déconnecté';
      }};

      worklet.port.onmessage = (e) => {{
        const {{ buffer, peak }} = e.data;
        const scale = 1 + Math.min(0.22, peak * 0.45);
        const spread = Math.round(Math.min(28, peak * 52));
        vuRing.style.transform = `scale(${{scale}})`;
        vuRing.style.boxShadow = muted ? 'none' : `0 0 ${{spread}}px ${{Math.round(spread/3)}}px var(--glow)`;
        if (!muted && ws && ws.readyState === WebSocket.OPEN) {{
          ws.send(buffer);
        }}
      }};
    }} catch (err) {{
      statusText.textContent = 'Accès micro refusé';
      latencyMeta.textContent = String(err && err.message ? err.message : err);
    }}
  }}

  mainBtn.addEventListener('click', () => {{
    if (!started) {{
      startMic();
      return;
    }}
    muted = !muted;
    mainBtn.classList.toggle('muted', muted);
    btnIcon.textContent = muted ? '🔇' : '🎙️';
    btnLabel.textContent = muted ? 'Réactiver' : 'Couper (Mute)';
    if (ws && ws.readyState === WebSocket.OPEN) {{
      ws.send(JSON.stringify({{ type: 'mute', muted }}));
    }}
  }});

  gainSlider.addEventListener('input', () => {{
    const pct = Number(gainSlider.value);
    gainVal.textContent = `${{pct}} %`;
    if (ws && ws.readyState === WebSocket.OPEN) {{
      ws.send(JSON.stringify({{ type: 'gain', value: pct / 100 }}));
    }}
  }});

  queueForm.addEventListener('submit', (e) => {{
    e.preventDefault();
    const query = queueInput.value.trim();
    if (!query || !ws || ws.readyState !== WebSocket.OPEN) return;
    ws.send(JSON.stringify({{ type: 'queue_add', query, singer_name: '{label}' }}));
    queueInput.value = '';
  }});
}})();
</script>
</body>
</html>"#,
        token_json = escape_js_inline_json(session_token)
    )
}

/// Serializes a string as a JSON literal safely escaped for inline HTML `<script>` blocks,
/// preventing `</script>` breakout / reflected XSS.
pub fn escape_js_inline_json(value: &str) -> String {
    let raw = serde_json::to_string(value).unwrap_or_else(|_| "\"\"".into());
    raw.replace('&', "\\u0026")
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('/', "\\/")
}

/// Renders the 1-tap iPhone certificate installation guide page (`/cert/install`).
pub const CERT_INSTALL_HTML: &str = r#"<!DOCTYPE html>
<html lang="fr">
<head>
<meta charset="utf-8" />
<meta name="viewport" content="width=device-width, initial-scale=1" />
<title>Certificat local — Karaok Party</title>
<style>
  body { background: #05070c; color: #f8fafc; font-family: -apple-system, sans-serif; padding: 28px 20px; line-height: 1.5; max-width: 520px; margin: auto; }
  h1 { font-size: 24px; margin-bottom: 8px; }
  .btn { display: block; text-align: center; background: #38bdf8; color: #020617; font-weight: 700; padding: 15px 18px; border-radius: 14px; text-decoration: none; margin: 20px 0; }
  ol { padding-left: 20px; color: #cbd5e1; }
  li { margin-bottom: 10px; }
</style>
</head>
<body>
  <h1>Activer le micro Safari iOS</h1>
  <p>Safari iOS exige une connexion HTTPS de confiance pour accéder au microphone en basse latence sur le Wi-Fi local.</p>
  <a class="btn" href="/cert/karaok-party-ca.pem">1. Télécharger le certificat local (.pem)</a>
  <ol>
    <li>Ouvrez <strong>Réglages → Général → VPN et gestion de l'appareil</strong> et installez le profil <em>Karaok Party Local CA</em>.</li>
    <li>Ouvrez <strong>Réglages → Général → Informations → Réglages des certificats</strong> et activez la confiance pour <em>Karaok Party Local CA</em>.</li>
    <li>Scannez à nouveau le QR Code <strong>Micro 1</strong> ou <strong>Micro 2</strong> sur votre Mac !</li>
  </ol>
</body>
</html>"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mobile_html_escapes_script_breakout_xss() {
        let html = render_mobile_mic_html(1, "</script><script>alert(1)</script>");
        assert!(!html.contains("</script><script>alert(1)</script>"));
        assert!(html.contains("\\u003c\\/script\\u003e"));
    }
}
