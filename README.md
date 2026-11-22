# Karaok Party

**Application de karaoké de qualité studio pour macOS (Apple Silicon M1/M2/M3/M4, macOS 14+)** — 100 % **Open Source (MIT)**, **gratuite** et **local-first** (zéro abonnement, fonctionnement hors-ligne complet).

Construit avec **Rust** + **Tauri v2** + **React 19 / TypeScript / Tailwind CSS**.

---

## Fonctionnalités Clés

- **Moteur Audio CoreAudio Temps Réel (< 15 ms)** : graphe audio strictement *lock-free* (`cpal` + `ringbuf`), décodage universel (`symphonia` MP3/FLAC/WAV/M4A/OGG), rééchantillonnage haute précision (`rubato`), transposition `-6` à `+6` demi-tons et tempo `70 %` à `130 %` (`signalsmith-stretch`).
- **2 Microphones iPhone Sans Installation (Mode Duo)** : scannez un QR Code depuis Safari iOS — zéro app à installer, zéro compte Apple Developer. Serveur HTTPS/WSS local (`axum` + `rustls` + `rcgen` + Bonjour `karaok-party.local`), `AudioWorklet` 128 échantillons (~2,67 ms), Adaptive Jitter Buffer (10–35 ms), VU-mètre et télécommande file d'attente sur iPhone.
- **Chaîne DSP Vocale Studio Indépendante (Mic 1 & Mic 2)** : passe-haut 80 Hz, Noise Gate anti-Larsen, Compresseur 3:1, détection de pitch YIN + interpolation parabolique, **Autotune temps réel synchronisé automatiquement à la tonalité du morceau**, réverbération stéréo algorithmique et préréglages (*Naturel, Studio, Scène/Live, Pop Star, Mode Expert*).
- **Séparation de Pistes IA Locale (4 / 6 Stems)** : architecture **HTDemucs v4** par segments de 7,8 s avec démarrage rapide dès 25 s traitées, cache persistant sur disque et reprise. Supporte les backends **Burn + Apple Metal**, **ONNX Runtime + CoreML** et un séparateur spectral pur Rust hors-ligne par défaut.
- **Paroles Synchronisées Mot par Mot & Éditeur** : récupération automatique **LRCLIB**, alignement forcé **`whisper-rs` (Metal)**, décalage en direct `[` / `]` (`±100 ms`), mode **Tap-to-Sync** (Barre Espace) et export Enhanced LRC.
- **Expérience Salon & Second Écran TV** : lecteur 5 lignes contrasté, compte à rebours 3 points, boucle A-B, curseur Voix Guide + mixeur 4 pistes, enregistrement multipiste avec réécoute & réglage voix/musique a posteriori, et **fenêtre dédiée TV / AirPlay plein écran**.
- **Import Local, YouTube & Playlists** : glisser-déposer audio, extraction YouTube haute qualité via `yt-dlp` + `ffmpeg`, import de métadonnées de playlists Spotify publiques.

---

## Architecture du Workspace

```text
karaok-party/
├── crates/
│   ├── karaok-core/         # Modèles métier, parser/sérialiseur Enhanced LRC, gammes & tonalités
│   ├── karaok-dsp/          # Chaîne DSP vocale zéro-allocation (HPF, Gate, Comp, YIN, Autotune, Reverb, JitterBuffer)
│   ├── karaok-audio/        # Moteur CoreAudio lock-free (cpal, ringbuf, symphonia, rubato, signalsmith-stretch, 4 stems, WAV recorder)
│   ├── karaok-mic-server/   # Serveur HTTPS/WSS iPhone (axum, rustls, rcgen, mdns-sd, qrcode) + WebApp Safari AudioWorklet
│   ├── karaok-ai/           # Pipeline HTDemucs chunked, client LRCLIB, alignement Whisper, importateur yt-dlp/ffmpeg
│   └── karaok-db/           # Base SQLite locale & migrations versionnées (sqlx)
├── src-tauri/               # Application native macOS Tauri v2 (IPC, événements 60Hz, fenêtre TV)
└── src/                     # Interface React 19 + TypeScript + Tailwind CSS (Studio + Mode TV)
```

---

## Prérequis de Compilation (macOS Apple Silicon)

- **macOS 14+ (Sonoma / Sequoia)** sur puce Apple Silicon (M1 / M2 / M3 / M4)
- **Xcode Command Line Tools** :
  ```bash
  xcode-select --install
  ```
- **Rust & Cargo** (`rustup`, Rust 1.85+) :
  ```bash
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
  ```
- **Node.js 20+** & **pnpm** :
  ```bash
  brew install node pnpm
  ```
- **Outils optionnels pour l'import YouTube & ML Metal natif** :
  ```bash
  brew install yt-dlp ffmpeg cmake
  ```

---

## Démarrage Rapide

```bash
# 1. Installer les dépendances frontend
pnpm install

# 2. Vérifier & exécuter toute la suite de tests Rust (DSP, Audio, Réseau, DB, IA)
cargo test --workspace

# 3. Lancer l'application desktop macOS en mode développement (Tauri v2)
pnpm tauri dev

# 4. Compiler le bundle macOS (.app / .dmg)
pnpm tauri build
```

### Backends IA : Exécution Hors-Ligne & Hooks Apple Metal / CoreML

Par défaut, Karaok Party embarque un **séparateur spectral STFT Mid-Side / Harmonique-Percussif 100 % pur Rust** (découpage en segments de 7,8 s, démarrage rapide à 25 s, cache persistant `manifest.json`) et un **aligneur vocal syllabique par enveloppe/onsets** — fonctionnant immédiatement hors-ligne sans téléchargement de poids externes ni CMake.

Les flags Cargo `karaok-ai/burn-metal`, `karaok-ai/ort-coreml` et `karaok-ai/whisper-metal` exposent les hooks d'extension de l'architecture `StemSeparatorBackend` / `WhisperAlignConfig` pour brancher ultérieurement les poids natifs HTDemucs v4 (`burn` Metal / `ort` CoreML) et Whisper GGML (`whisper-rs` Metal, nécessite `cmake`).

---

## Guide d'Utilisation : Appairer 1 ou 2 iPhones en Micro Sans Fil

1. Connectez votre Mac et votre/vos iPhone(s) au **même réseau Wi-Fi local**.
2. Cliquez sur **Voix & Micros** dans la barre inférieure de Karaok Party.
3. Lors de la première utilisation sur un iPhone, scannez ou ouvrez le lien **Installer le certificat local** (`karaok-party-ca.pem`), puis sur l'iPhone :
   - *Réglages -> Général -> VPN et gestion de l'appareil -> Karaok Party Local CA -> Installer*
   - *Réglages -> Général -> Informations -> Réglages des certificats -> Activer « Karaok Party Local CA »*
4. Scannez le **QR Code Micro 1 (Bleu)** ou **Micro 2 (Corail)** avec l'appareil photo de l'iPhone, autorisez l'accès au micro dans Safari et chantez !
5. Si votre Mac est relié à une barre de son ou une enceinte Bluetooth, ajustez le curseur **Calibration Latence (0–250 ms)** dans le tiroir Voix & Micros.

---

## Documentation Technique

- [`TECHNICAL_SPECIFICATIONS.md`](./TECHNICAL_SPECIFICATIONS.md) — Architecture complète, graphe *lock-free*, protocole WSS binaire, pipeline IA et schéma SQLite.
- [`AI_PROMPT_MASTER.md`](./AI_PROMPT_MASTER.md) — Invariants temps réel audio et règles d'ingénierie.

## Licence

Distribué sous licence **MIT**. Voir [`LICENSE`](./LICENSE).
