# AI PROMPT MASTER — RÈGLES D'INGÉNIERIE KARAOK PARTY

Ce document définit les règles non négociables que tout agent IA ou contributeur doit respecter lorsqu'il intervient sur **Karaok Party**.

---

## 1. Invariants Temps Réel Audio (ZERO TOLERANCE)

Dans tout code exécuté depuis le callback CoreAudio (`cpal`) ou appelé par celui-ci (`crates/karaok-dsp`, boucle de rendu de `crates/karaok-audio`) :

1. **INTERDICTION D'ALLOCATION MÉMOIRE** : aucun `Vec::new()`, `Vec::push()`, `String`, `format!()`, `Box::new()`, `HashMap`, ni redimensionnement dynamique. Tous les tampons (delay lines, fenêtres YIN, lignes de réverbération, buffers scratch) doivent être pré-alloués lors de l'initialisation (`new()` / `prepare()`).
2. **INTERDICTION DE VERROUS BLOQUANTS** : aucun `std::sync::Mutex`, `RwLock`, `Condvar`, `tokio::sync::*`, ni canal bloquant. Utiliser exclusivement :
   - `ringbuf` (SPSC *lock-free*) pour les flux audio PCM ;
   - `std::sync::atomic::*` (`AtomicU32`, `AtomicU64`, `AtomicI32`, `AtomicBool`) avec `Ordering::Relaxed` / `Acquire` / `Release` pour les paramètres et la télémétrie.
3. **INTERDICTION D'APPELS SYSTÈME ET DE LOGS** : pas de lecture/écriture disque, pas de réseau, pas de `println!`, `eprintln!`, ni `tracing::info!` dans le callback audio.
4. **INTERDICTION DE `unwrap()` / `expect()` / `panic!()`** : aucun chemin paniquant dans le thread audio. Toujours utiliser des accès bornés (`.get()`, `.clamp()`, `.unwrap_or(0.0)`) et filtrer les valeurs non finies (`if !sample.is_finite() { 0.0 }`).
5. **ANTI-DÉNORMAUX & SÉCURITÉ ACOUSTIQUE** : protéger les filtres récursifs contre les nombres dénormaux et appliquer un soft-limiter en sortie master pour éviter tout écrêtage brutal dans les enceintes ou casques.

---

## 2. Conventions Rust

- **Édition Rust** : `2021` (MSRV `1.85+`, Apple Silicon `aarch64-apple-darwin`).
- **Gestion d'erreurs** :
  - Crates bibliothèque (`karaok-*`) : `thiserror` avec des enums d'erreur typés et explicites.
  - Application binaire (`src-tauri`) : conversion propre vers des erreurs sérialisables Tauri (`String` ou struct JSON structurée).
- **Async / Tokio** :
  - Ne jamais bloquer le runtime Tokio (`std::thread::sleep` ou décodage/inférence lourde sur un worker Tokio -> utiliser `tokio::task::spawn_blocking` ou des threads OS dédiés).
  - Ne jamais conserver de verrou à travers un `.await`.
- **Sécurité & Sous-processus** :
  - Toujours passer les arguments à `yt-dlp` et `ffmpeg` via `Command::arg(...)`, jamais par interpolation shell (`sh -c`).
  - Valider strictement les schémas d'URL (`https://`) et empêcher toute traversée de répertoire (*path traversal*) lors de la lecture des caches de stems ou enregistrements.
  - Ne jamais contourner de DRM (Spotify : import de métadonnées publiques uniquement).
- **Qualité & Lint** :
  - `cargo check --workspace` et `cargo test --workspace` doivent passer avec zéro erreur.
  - Éviter les warnings compilateur inutiles et documenter chaque API publique (`///`).

---

## 3. Conventions TypeScript / React 19 / Tailwind CSS

- **TypeScript strict** (`strict: true`, zéro `any` implicite, DTOs centralisés dans `src/types/karaok.ts` alignés 1:1 avec les structs Rust `#[serde(rename_all = "camelCase")]`).
- **Performance 60–120 FPS sur la scène karaoké** :
  - Ne pas re-rendre toute l'application à 60 Hz : isoler le temps courant de lecture / coloration mot par mot et les VU-mètres dans des abonnements granulaires ou `requestAnimationFrame` interpolés sur l'horloge audio.
- **Lisibilité Salon & Accessibilité** :
  - Contraste WCAG AA/AAA sur fond sombre OLED (`#090A0F`), typographie haute lisibilité à 3–5 mètres pour les paroles.
  - Support clavier complet (`Espace` Lecture/Pause ou Tap-to-Sync, `[` / `]` décalage paroles `±100 ms`, `Flèche Gauche` recul 5s, `Échap` fermeture tiroirs).
  - Respect de `prefers-reduced-motion`.
- **États d'interface complets** : chaque vue (bibliothèque, file d'attente, micros, paroles, enregistrements) doit gérer proprement les états *vide*, *chargement*, *progression* et *erreur explicite*.

---

## 4. Checklist de Validation Obligatoire Avant Toute Clôture

Avant de déclarer une phase ou une PR terminée, exécuter et vérifier **avec preuves fraîches** :

- [ ] `cargo test --workspace` : 100 % des tests unitaires & intégration DSP/Audio/Réseau/DB/AI verts.
- [ ] `cargo check --workspace` (incluant `src-tauri`) : compilation complète sans erreur sur macOS Apple Silicon.
- [ ] `pnpm run typecheck` (`tsc --noEmit`) : zéro erreur TypeScript.
- [ ] `pnpm run build` : build production Vite (fenêtre principale + fenêtre TV) vert.
- [ ] Audit des chemins temps réel : aucun `unwrap()`, `expect()`, `Mutex`, ni allocation dans les fonctions `process_*` / callback `cpal`.
- [ ] Audit sécurité : génération TLS `rcgen` locale, jeton d'appairage session sur `/ws/mic/:id`, zéro secret en dur, zéro appel shell non échappé.
- [ ] Vérification visuelle/fonctionnelle de l'interface (scène 5 lignes, tiroir micros, dock studio, éditeur sync, fenêtre TV).
