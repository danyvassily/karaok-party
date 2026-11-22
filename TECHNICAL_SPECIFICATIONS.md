# TECHNICAL SPECIFICATIONS — KARAOK PARTY

> **Version** : 1.0.0  
> **Cible** : macOS 14+ (Apple Silicon M1 / M2 / M3 / M4, `aarch64-apple-darwin`)  
> **Licence** : MIT (Open Source, 100 % Local-First)  
> **Architecture** : Tauri v2 (Backend 100 % Rust + Frontend React 19 / TypeScript / Tailwind CSS v4)

---

## 1. Vue d'ensemble de l'architecture

Karaok Party sépare strictement les domaines critiques en crates Rust spécialisées au sein d'un workspace Cargo unique, orchestrées par le binaire Tauri v2 (`src-tauri`) :

```text
+---------------------------------------------------------------------------------+
|                         FRONTEND (React 19 + TypeScript)                        |
|   [ Main Studio Window (120 FPS) ]          [ TV / AirPlay Second Screen ]      |
|   Lyrics 5-Lines | Sidebar | Dock | Sync    Lyrics 5-Lines | Next Singer | HUD  |
+------------------------------+-----------------------------------+--------------+
                               | Tauri v2 IPC (Commands + 60Hz Events)
+------------------------------v-----------------------------------v--------------+
|                         TAURI CORE ORCHESTRATOR (src-tauri)                     |
+-------+----------------+----------------+-----------------+---------------------+
|       |                |                |                 |                     |
| karaok-core      karaok-audio      karaok-dsp      karaok-mic-server  karaok-ai |
| - Domain DTOs    - cpal CoreAudio  - HPF 80Hz      - axum + rustls    - HTDemucs|
| - Enhanced LRC   - ringbuf SPSC    - Noise Gate    - rcgen Local CA   - Chunking|
| - Scale/Key Math - symphonia       - Compressor    - mdns-sd Bonjour  - LRCLIB  |
|                  - rubato          - YIN/MPM Pitch - QR SVG (/mic/1,2)- Whisper |
|                  - signalsmith     - Scale Autotune- AudioWorklet App - yt-dlp  |
|                  - 4-Stem Mixer    - Dattorro Rev. - Adaptive Jitter  - Spotify |
|                  - WAV Recorder    - Vocal Presets   Buffer (10-35ms)   meta    |
+-------+----------------+----------------+-----------------+-----------+---------+
        |                                                               |
        +---------------------------+-----------------------------------+
                                    v
                       [ karaok-db (SQLite / sqlx) ]
                       ~/Library/Application Support/KaraokParty/
```

---

## 2. Graphe Audio Temps Réel *Lock-Free*

### 2.1 Contraintes temps réel CoreAudio
- **Fréquence d'échantillonnage maître** : `48 000 Hz` (stéréo entrelacé `f32`, 2 canaux en sortie).
- **Taille de tampon CoreAudio configurable** : `64`, `128` (défaut, ~2,67 ms) ou `256` échantillons.
- **Objectif de latence aller-retour (iPhone -> Wi-Fi -> DSP -> CoreAudio)** : `< 15 ms` sur Wi-Fi 5/6 local.
- **Règle d'or absolue dans le callback CoreAudio** :
  - Zéro allocation mémoire (`malloc`/`Vec::push`/`String`/`Box`).
  - Zéro verrou bloquant (`Mutex`, `RwLock`, `Condvar`, `tokio::sync`).
  - Zéro appel système (I/O disque, réseau, logs synchrones).
  - Zéro `unwrap()` / `expect()` / `panic!()`.

### 2.2 Schéma de flux inter-threads *lock-free*

```text
[Thread Décodage / Stems]                       [Serveur WebSocket Mic 1 & Mic 2]
 symphonia -> rubato (48kHz)                     WSS Binary Frame (128 samples)
 -> signalsmith-stretch (Pitch/Tempo)            -> Adaptive Jitter Buffer (10-35ms)
        |                                                |             |
        | 4x SPSC RingBuf (Vocals/Drums/Bass/Other)      | SPSC Mic 1  | SPSC Mic 2
        v                                                v             v
+=================================================================================+
|                   CALLBACK TEMPS RÉEL COREAUDIO (cpal, 48 kHz)                  |
|                                                                                 |
|  1. Lire AtomicParams (volumes stems, key, tempo, A-B loop, calibration delay)  |
|  2. Consommer Stems RingBuf -> Mixeur 4 pistes (Vocal Guide 0..100% + Instru)   |
|  3. Consommer Mic 1 RingBuf -> VocalDspChain[0] (HPF->Gate->Comp->Tune->Reverb) |
|  4. Consommer Mic 2 RingBuf -> VocalDspChain[1] (HPF->Gate->Comp->Tune->Reverb) |
|  5. Delay Line Calibration (0..250 ms) sur bus instrumental ou retour vocal     |
|  6. Sommation Master + Soft Limiter (tanh/hard-clip guard) -> Sortie CoreAudio  |
|  7. Pousser (Instru L/R, Mic1, Mic2) vers SPSC RingBuf Enregistreur (non-block) |
|  8. Publier télémétrie atomique (horloge échantillon, VU-mètres, pitch Hz/cents)|
+=================================================================================+
        |                                                |
        v (60 Hz polling UI, lock-free Atomics)          v [Thread Enregistrement]
   [Tauri Event Emitter]                          Écriture WAV multi-pistes (hound)
                                                  pour réécoute & post-mix voix/musique
```

---

## 3. Chaîne DSP Vocale Indépendante (Mic 1 & Mic 2)

Chaque canal micro (`MicChannelId::Mic1`, `MicChannelId::Mic2`) possède sa propre instance `VocalDspChain` pré-allouée avant le démarrage du flux audio :

1. **Filtre Passe-Haut (HPF Biquad 2e ordre Butterworth à 80 Hz)** : supprime les bruits de manipulation du boîtier iPhone et les plosives graves.
2. **Noise Gate Intelligent** : détecteur d'enveloppe RMS + hystérésis (seuil d'ouverture / fermeture réglable en dBFS, attaque 2 ms, maintien 40 ms, relâchement 80 ms) pour empêcher la reprise des enceintes (anti-Larsen).
3. **Compresseur Dynamique Doux** : ratio `3:1`, seuil `-18 dBFS`, genou doux (*soft knee* 6 dB), attaque `5 ms`, relâchement `90 ms`, gain de compensation automatique (*makeup gain*).
4. **Détecteur de Hauteur Temps Réel (YIN + Interpolation Parabolique)** :
   - Fenêtre glissante pré-allouée de `1024` échantillons (`21,3 ms` à 48 kHz), mise à jour par pas de `128` échantillons.
   - Plage vocale cible : `75 Hz` (D2) à `1100 Hz` (C#6).
   - Sortie : fréquence détectée `f0` (Hz), confiance (`0.0..1.0`), note MIDI la plus proche et déviation en centièmes de demi-ton (`-50..+50 cents`).
5. **Correcteur de Justesse (Autotune lié à la tonalité du morceau)** :
   - Table de gamme active (`Chromatic`, `Major`, `Minor`) automatiquement transposée de `song_key + transpose_semitones` à chaque changement du curseur tonalité.
   - Lissage exponentiel du ratio de pitch piloté par `retune_speed_ms` (`0 ms` effet Pop Star / T-Pain -> `120 ms` correction transparente).
   - Pitch-shifting basse latence par ligne à retard modulée double-tête avec fenêtrage Hann synchronisé (PSOLA / phase-coherent dual-tap delay) préservant la continuité de phase sans clic, avec dosage `dry_wet` (`0..100 %`).
6. **Réverbération Stéréo Algorithmique (Freeverb / Dattorro Plate pur Rust)** :
   - 8 filtres peigne parallèles accordés en stéréo (spread 23 échantillons) + 4 filtres passe-tout en série par canal, paramètres `room_size`, `damping`, `wet`.
7. **Préréglages Vocaux Instantanés** :
   - `Natural` : compression douce, réverbération subtile (`12%`), autotune désactivé (`0%`).
   - `Studio` : compression chaleureuse, autotune naturel (`85 ms`, `55%`), Plate Reverb claire (`24%`).
   - `Live` : réverbération ample (`42%`, grande salle), autotune modéré (`50 ms`, `65%`).
   - `PopStar` : autotune rapide (`10 ms`, `95%`), brillance EQ (+3 dB high-shelf 6 kHz), réverbération moderne (`30%`).
   - `Expert` : contrôle manuel granulaire de tous les paramètres par micro.

---

## 4. Protocole Réseau Sécurisé iPhone (HTTPS / WSS / mDNS)

### 4.1 Contexte Sécurisé Safari iOS
Safari iOS bloque `navigator.mediaDevices.getUserMedia` et `AudioWorklet` sur HTTP non chiffré. Karaok Party embarque donc :
1. **Autorité de Certification Locale (`rcgen`)** :
   - Générée une seule fois et stockée dans `~/Library/Application Support/KaraokParty/certs/ca.pem` & `ca.key`.
   - Certificat serveur signé par cette CA avec SAN : `DNS:karaok-party.local`, `DNS:localhost`, `IP:<LAN_IP>`, `IP:127.0.0.1`.
   - Route `/cert/karaok-party-ca.pem` (et `/cert/install`) permettant l'installation en 1 tap sur iOS (*Réglages -> Profil téléchargé -> Certificats de confiance*).
2. **Découverte mDNS / Bonjour (`mdns-sd`)** :
   - Annonce du service `_https._tcp.local.` pointant vers `karaok-party.local:8443` + résolution de l'adresse IP LAN en repli direct.
3. **Appairage QR Code (`qrcode`)** :
   - Génération SVG instantanée pour `https://<host>:8443/mic/1?token=<session>` (Micro 1 — Bleu) et `https://<host>:8443/mic/2?token=<session>` (Micro 2 — Corail).

### 4.2 Protocole Binaire WebSocket (`wss://<host>:8443/ws/mic/:id`)
Pour garantir une surcharge minimale et zéro sérialisation JSON sur le flux audio :

| Offset (octets) | Taille | Type (Little-Endian) | Description |
| :--- | :--- | :--- | :--- |
| `0` | 1 | `u8` | Magic / Version (`0x4B` = `'K'`, v1) |
| `1` | 1 | `u8` | Format (`1` = `Int16` PCM, `2` = `Float32` PCM) |
| `2` | 2 | `u16` | Nombre d'échantillons (typiquement `128`) |
| `4` | 4 | `u32` | Numéro de séquence monotone (`seq`) |
| `8` | 8 | `u64` | Horodatage client en microsecondes (`timestamp_us`) |
| `16` | `N * 2` ou `N * 4` | `[i16]` / `[f32]` | Trame audio mono 48 kHz (128 échantillons ≈ 2,67 ms) |

Les messages **Texte JSON** sur le même WebSocket transportent les commandes télécommande :
- Client -> Mac : `{ "type": "mute", "muted": true }`, `{ "type": "gain", "value": 1.0 }`, `{ "type": "queue_add", "query": "..." }`, `{ "type": "ping", "t": 12345 }`.
- Mac -> Client : `{ "type": "telemetry", "vu_db": -14.2, "jitter_ms": 14.0, "connected": true }`, `{ "type": "pong", "t": 12345 }`.

### 4.3 Adaptive Jitter Buffer (Rust)
- Tampon circulaire dédié par micro ajustant dynamiquement sa cible de pré-remplissage entre **10 ms (480 échantillons)** et **35 ms (1680 échantillons)** selon l'écart-type des intervalles d'arrivée (*RFC 3550 jitter estimator*).
- En cas de paquet manquant (`seq` sauté) : fondu de dissimulation (*Packet Loss Concealment* simple par décroissance douce sur 64 échantillons) sans clic audible.

---

## 5. Pipeline IA 100 % Local (HTDemucs v4, LRCLIB & Whisper-rs)

### 5.1 Séparation de Pistes par Segments (Chunked Streaming Separation)
- **Moteurs supportés** :
  - **HTDemucs v4 via `burn` + backend Apple Metal** (`--features burn-metal`) : modèles `htdemucs` (4 pistes : *vocals, drums, bass, other*) et `htdemucs_6s` (6 pistes : + *guitar, piano*).
  - **ONNX Runtime + CoreML (`ort`)** (`--features ort-coreml`) : exécution ANE/GPU Apple Silicon.
  - **Séparateur Spectral Mid-Side / Harmonique-Percussif Pur Rust (défaut hors-ligne sans poids externes)** : garantit un fonctionnement immédiat dès l'installation même avant le téléchargement des poids HTDemucs (~170 Mo).
- **Découpage & Reprise** :
  - Fenêtres de **7,8 secondes** avec overlap-add de `0,5 s` (fenêtre triangulaire/Hann) pour éviter tout artefact de jonction.
  - **Démarrage rapide (Fast-Start)** : dès que les **25 premières secondes** (4 segments) sont écrites sur disque, l'état passe à `ReadyToPlay` et l'utilisateur peut lancer le morceau pendant que les segments suivants continuent en arrière-plan.
  - **Cache persistant & reprise** : `~/Library/Application Support/KaraokParty/stems/<track_hash>/` contenant `manifest.json` (segments complétés, modèle utilisé) + `vocals.wav`, `drums.wav`, `bass.wav`, `other.wav`.

### 5.2 Synchronisation des Paroles & Alignement IA
1. **LRCLIB (`https://lrclib.net/api/get` & `/api/search`)** : récupération automatique des paroles `.lrc` et Enhanced LRC mot par mot (`[mm:ss.xx] <mm:ss.xx> Mot`).
2. **Alignement Forcé Mot par Mot (`whisper-rs` Metal)** :
   - Sur la piste `vocals.wav` isolée par Demucs, aligne des paroles brutes non synchronisées pour générer un fichier Enhanced LRC mot par mot.
   - Repli pur Rust basé sur la détection d'enveloppe/onsets syllabiques de la piste vocale lorsque le modèle Whisper GGML n'est pas encore installé.

### 5.3 Import YouTube & Spotify (Métadonnées uniquement)
- **YouTube** : sous-processus contrôlé `yt-dlp -x --audio-format wav --audio-quality 0` + `ffmpeg` (validation stricte de l'URL, zéro injection shell via `Command::arg`), import direct dans la bibliothèque locale + déclenchement de la séparation Demucs.
- **Spotify** : conformément aux conditions d'utilisation et à l'absence de contournement DRM, seul l'import de **métadonnées publiques** (titre / artiste d'une playlist publique via oEmbed/page publique) est supporté pour résoudre des correspondances locales ou YouTube.

---

## 6. Modèle de Données SQLite (`karaok-db`)

Base de données locale gérée par `sqlx` avec migrations versionnées (`crates/karaok-db/migrations/`) :

- **`tracks`** : `id`, `title`, `artist`, `album`, `duration_ms`, `source_type` (`local` | `youtube`), `source_path`, `source_url`, `content_hash`, `musical_key`, `bpm`, `favorite_transpose` (`-6..+6`), `is_favorite`, `stem_status` (`pending` | `streaming_ready` | `completed` | `failed`), `stem_progress` (`0..100`), `stem_model`, `stems_dir`, `created_at`, `updated_at`.
- **`lyrics`** : `track_id` (PK/FK), `format` (`plain` | `lrc` | `enhanced_lrc`), `has_word_sync`, `offset_ms`, `content`, `source` (`lrclib` | `whisper` | `manual`), `updated_at`.
- **`queue_items`** : `id`, `track_id` (FK), `position`, `mode` (`solo` | `duo`), `singer_1`, `singer_2`, `status` (`queued` | `playing` | `completed`), `added_by`, `created_at`.
- **`recordings`** : `id`, `track_id` (FK), `title`, `created_at`, `duration_ms`, `instrumental_path`, `mic1_path`, `mic2_path`, `mix_vocal_balance`, `exported_path`.
- **`app_settings`** : clé/valeur JSON (taille buffer CoreAudio, calibration latence ms, préréglages Mic 1/2, modèle Demucs).

---

## 7. Interface Utilisateur & Mode Second Écran TV

- **Fenêtre Principale (`main`)** :
  - **Scène Centrale** : lecteur 5 lignes (2 passées 30 %, ligne active coloration fluide mot par mot à 60-120 FPS pilotée par l'horloge échantillon CoreAudio, 2 à venir 65 %), compte à rebours 3 points avant reprise vocale, jauge de justesse en direct désactivable.
  - **Panneau Gauche Escamotable** : Bibliothèque (recherche, filtres, drag & drop, import YouTube, progression Demucs) et File d'attente Duo/Solo.
  - **Dock Studio Inférieur** : Transport, `-5s`, Boucle A-B, Tonalité (`-6..+6`), Curseur Voix Guide + Mixeur 4 pistes, Enregistrer/Réécouter, Tiroir Voix & Micros, Mode Second Écran TV.
  - **Mode Sync Paroles** : LRCLIB, Whisper alignement auto, décalage `[` / `]` (`±100 ms`), Tap-to-Sync (Barre Espace) avec sauvegarde Enhanced LRC.
- **Fenêtre Secondaire TV (`tv`)** :
  - Fenêtre Tauri v2 dédiée (`tv.html`) synchronisée en temps réel via les événements Tauri, déplaçable sur téléviseur HDMI / AirPlay en plein écran, affichant uniquement les 5 lignes de paroles, le compte à rebours, le titre en cours et le prochain chanteur.
