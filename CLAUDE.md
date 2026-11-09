# CLAUDE.md

**MagicMus** — application karaoké Windows en Rust : télécharge une vidéo YouTube, retire la voix par IA, affiche les paroles
synchronisées mot à mot et permet de changer tonalité et vitesse en direct. Interface en français, code commenté en
français. Voir aussi `README.md`.

## Commandes

```bash
cargo build --release          # target/release/karaoke.exe (GUI) et karaoke-cli.exe
cargo test --release           # tests unitaires
# test d'intégration de l'export (ffmpeg + un karaoké existant) :
KARAOKE_TEST_FILE=<karaoke_ia.mp4> cargo test --release --test export -- --ignored
```

Prérequis : Rust (MSVC) et Visual Studio Build Tools (C++ requis par `build.rs`, qui compile Signalsmith Stretch
et intègre l'icône `assets/magicmus.ico` via winresource).
Pas de libclang : les liaisons FFI de Signalsmith sont écrites à la main dans `src/stretch.rs`.

L'exe livré est `dist\MagicMus.exe` (copie de `target/release/karaoke.exe`).

Vérifier le rendu des écrans sans lancer de traitement (option de compilation `demo`, absente de l'exe livré) :

```bash
CARGO_TARGET_DIR=target/demo cargo build --release --features demo --bin karaoke
MAGICMUS_DEMO=traitement target/demo/release/karaoke.exe   # ou MAGICMUS_DEMO=correction
```

Le build de débogage (`cargo build` sans `--release`) plante au démarrage (erreur de segmentation, cause non
identifiée, aussi sans le mode démo) : tester avec `--release`.

## Architecture

| Fichier | Rôle |
|---|---|
| `src/main.rs` | GUI egui/eframe 0.37 (`App::ui`, `Panel::left`, `CentralPanel::show(ui, …)`) : écrans accueil, traitement, lecteur, modale de correction |
| `src/theme.rs` | design MagicMus (maquette Claude Design) : couleurs, polices Nunito / DM Serif Display, composants dessinés à la main |
| `src/bin/karaoke-cli.rs` | ligne de commande (clap) |
| `src/pipeline.rs` | enchaînement : outils → téléchargement → décodage → séparation → Whisper → encodage → paroles |
| `src/setup.rs` | téléchargement automatique (yt-dlp, ffmpeg, Deno, ONNX Runtime, modèles) dans `%LOCALAPPDATA%\Karaoke` |
| `src/mdx.rs` | séparation MDX-Net (UVR Inst HQ 3) via `ort` en chargement dynamique (`onnxruntime.dll`) |
| `src/dsp.rs` | méthode « rapide » : masque spectral du canal central |
| `src/lyrics.rs` | Whisper (whisper.cpp `-ml 1 -sow`), lignes, sous-titres ASS `\kf` |
| `src/lrclib.rs` | paroles officielles synchronisées (LRCLIB) |
| `src/align.rs` | alignement Needleman-Wunsch des paroles exactes sur les mots reconnus |
| `src/player.rs` | lecteur : cpal + Signalsmith (tonalité/vitesse), frames vidéo ffmpeg, rendu des paroles, bascule voix originale |
| `src/export.rs` | rendu final : tonalité/vitesse, `setpts`, paroles incrustées (`ass`) |
| `src/tools.rs` | lancement des outils sans console, PATH, lecture de progression |

Dossier de travail d'une chanson : `source.*` (original), `karaoke_ia.mp4` / `karaoke_rapide.mp4`, `whisper.json`
(résultat brut, réutilisé pour réaligner), `paroles.json`, `titre.txt`, `source.txt`.

## Décisions et contraintes

- **Aucune installation pour l'utilisateur** : tout outil manquant est téléchargé au premier usage ; ne pas ajouter de
  dépendance système (Python, etc.).
- ONNX Runtime en `load-dynamic` : la version statique pour Windows inclut DirectML et exige `DXCORE.lib`, absent du
  SDK Windows 10.0.18362 installé.
- Paroles : texte saisi > LRCLIB > reconnaissance Whisper seule ; le minutage mot à mot vient de Whisper.
- L'horloge du lecteur est la position audio ; vidéo et paroles la suivent (`Player::position` compense la latence).
- Les tests utilisent des paroles factices (« un deux trois… ») : ne jamais versionner ni afficher de vraies paroles.
- Polices Nunito (fontsource) : leurs noms de famille internes sont inhabituels (« Nunito ExtraLight ExtraBold ») ;
  les sous-titres exportés citent ce nom exact (`lyrics::ASS_FONT`), sinon libass retombe sur Arial.
- Captures d'écran de test : utiliser `PrintWindow` sur la fenêtre de l'app, pas une capture de l'écran (qui peut
  montrer ce que l'utilisateur fait par-dessus).

## État (2026-11-09)

- Vérifié : séparation IA (comparée à une référence Python, 31 dB), export, lecteur, bascule voix originale
  (pistes alignées à l'échantillon), recherche LRCLIB + alignement sur LRC.
- **Non vérifié par Claude** : l'exécution de whisper.cpp (binaire téléchargé, refusé par le contrôle de sécurité).
  L'utilisateur a jugé la reconnaissance seule mauvaise, d'où LRCLIB et l'éditeur de paroles.
- Piste en attente : version web sur un serveur Proxmox (i5-7500, 4 cœurs, ~5 Go libres) avec lecteur dans le
  navigateur (Signalsmith en WASM).
