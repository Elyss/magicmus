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
MAGICMUS_DEMO=resynchro target/demo/release/karaoke.exe <karaoke_ia.mp4>   # bouton « Resynchroniser »
```

Diagnostic de synchronisation d'une chanson (minutages et compteurs seulement, jamais le texte) :

```bash
cargo run --release --example diag_paroles -- "<dossier de la chanson>" [--resume]
```

Le build de débogage (`cargo build` sans `--release`) plante au démarrage (erreur de segmentation, cause non
identifiée, aussi sans le mode démo) : tester avec `--release`.

## Architecture

| Fichier | Rôle |
|---|---|
| `src/main.rs` | GUI egui/eframe 0.37 (`App::ui`, `Panel::left`, `CentralPanel::show(ui, …)`) : écrans accueil, traitement, lecteur, modale de correction |
| `src/theme.rs` | design MagicMus (maquette Claude Design) : couleurs, polices Nunito / DM Serif Display, composants dessinés à la main |
| `src/ui_library.rs` | onglets Créer / Bibliothèque / Historique, réglages (`reglages.json`), déplacement depuis `Musique\Karaoke` |
| `src/library.rs` | bibliothèque (dossiers imbriqués), historique, emplacement `Documents\MagicMus` (`MAGICMUS_DATA` pour le changer) |
| `src/bin/karaoke-cli.rs` | ligne de commande (clap) |
| `src/pipeline.rs` | enchaînement : outils → téléchargement → décodage → séparation → Whisper → encodage → paroles |
| `src/setup.rs` | téléchargement automatique (yt-dlp, ffmpeg, Deno, ONNX Runtime, modèles) dans `%LOCALAPPDATA%\Karaoke` |
| `src/mdx.rs` | séparation MDX-Net (UVR Inst HQ 3) via `ort` en chargement dynamique (`onnxruntime.dll`) |
| `src/dsp.rs` | méthode « rapide » : masque spectral du canal central |
| `src/lyrics.rs` | Whisper (whisper.cpp `-ml 1 -sow`), lignes, sous-titres ASS `\kf` |
| `src/lrclib.rs` | paroles officielles synchronisées (LRCLIB) |
| `src/forced.rs` | alignement forcé MMS (émissions CTC via `ort`, Viterbi), méthode principale du minutage |
| `src/align.rs` | secours : alignement Needleman-Wunsch des paroles exactes sur les mots reconnus par Whisper |
| `src/vocal.rs` | attaques de la voix isolée (`attaques.json`) |
| `src/player.rs` | lecteur : cpal + Signalsmith (tonalité/vitesse), frames vidéo ffmpeg, rendu des paroles, bascule voix originale |
| `src/export.rs` | rendu final : tonalité/vitesse, `setpts`, paroles incrustées (`ass`) |
| `src/tools.rs` | lancement des outils sans console, PATH, lecture de progression |

Dossier de travail d'une chanson : `source.*` (original), `karaoke_ia.mp4` / `karaoke_rapide.mp4`, `whisper.json`
(résultat brut, réutilisé pour réaligner), `paroles.json`, `attaques.json`, `emissions.bin` (sorties MMS, ≈ 0,4 Mo par minute), `titre.txt`, `source.txt`.

## Décisions et contraintes

- **Aucune installation pour l'utilisateur** : tout outil manquant est téléchargé au premier usage ; ne pas ajouter de
  dépendance système (Python, etc.).
- ONNX Runtime en `load-dynamic` : la version statique pour Windows inclut DirectML et exige `DXCORE.lib`, absent du
  SDK Windows 10.0.18362 installé.
- Paroles : texte saisi > LRCLIB > reconnaissance Whisper seule. Construction dans `pipeline::build_lyrics`
  (utilisée par le traitement, le bouton « Resynchroniser » et « Appliquer » de la correction ; ne relance pas
  Whisper : relit `whisper.json`).
- Minutage mot à mot : **alignement forcé MMS** (`src/forced.rs`, `setup::ensure_aligner`) — modèle ONNX fp16
  `tatsu020/mms-300m-fa-onnx` (CC-BY-NC 4.0, non commercial, signalé dans le README), entrée 16 kHz mono de la voix
  isolée (source − karaoké), sortie log-probabilités par trame de 20 ms sur 31 symboles (0 = vide, 4..30 = a–z et
  apostrophe ; accents retirés), fenêtres de 15 s + 1 s de contexte, Viterbi CTC avec un **joker** gratuit entre
  les vers (absorbe chœurs, rires, répétitions absentes du texte). **Guide** : si le résultat de secours contient
  au moins 10 mots entendus par Whisper (`MIN_GUIDE_ANCHORS`), chaque vers est cherché à ±4 s de sa place
  (`GUIDE_MARGIN_S`) ; sans guide, MMS se trompait de plusieurs dizaines de secondes sur Feel Good Inc. (LRC d'une
  autre version, décalée de 17 à 40 s, et Whisper muet sur les 90 premières secondes). Résultat en cache dans
  `emissions.bin`. Garde-fou `pipeline::agrees` : si plus de 30 % des vers démarrent à plus de 2,5 s de la méthode
  de secours (Whisper + `align.rs`), c'est la méthode de secours qui est retenue.
- Exactitude vérifiée sur parole de synthèse (System.Speech, heure exacte de chaque mot) :
  `cargo run --release --example diag_paroles -- --tts <wav 16 kHz> <json {text, words:[{text,start}]}>` →
  écart médian 0,03 s (en), 0,065 s (fr), toujours en retard, 100 % / 98 % ≤ 0,2 s. Sur chant, MMS place les mots
  en moyenne 0,24 s après Whisper (dont les heures de jetons sont moins fiables) : pas de vérité terrain sur chant.
  Test de robustesse : `tts_extra.ps1` (scratchpad, non versionné) fait dire des phrases absentes du texte ;
  pire mot 3,9 s → 1,4 s (fr) et 1,2 s → 0,11 s (en) grâce au joker. `--mms --lignes` : relevé par vers ;
  `--libre` : sans guide.
  `--ecrire` refait paroles.json d'une chanson comme « Resynchroniser » (sauvegarde `paroles.avant-mms.json`).
- Whisper ne transcrit qu'environ 60 % des mots chantés (interjections, mots courts, répétitions manquent). Les mots
  non reconnus sont placés sur les **attaques de la voix** (`src/vocal.rs`, montées d'énergie de la voix isolée,
  conservées dans `attaques.json`) en début de vers et dans les longs intervalles, sinon répartis dans l'intervalle.
  Réglages choisis au banc d'essai à mots masqués : `cargo run --release --example diag_paroles -- <dossier> --banc debut|suites [--seuils]`.
- Les heures LRCLIB viennent souvent d'une autre version du morceau : `align::recalibrate` mesure l'écart
  (voix entendue − LRC) vers par vers et le lisse avant l'alignement ; `enforce_order` interdit mots désordonnés
  et chevauchements (sinon plusieurs mots « s'allument » en même temps).
- L'horloge du lecteur est la position audio ; vidéo et paroles la suivent (`Player::position` compense la latence).
- Les tests utilisent des paroles factices (« un deux trois… ») : ne jamais versionner ni afficher de vraies paroles.
- Polices Nunito (fontsource) : leurs noms de famille internes sont inhabituels (« Nunito ExtraLight ExtraBold ») ;
  les sous-titres exportés citent ce nom exact (`lyrics::ASS_FONT`), sinon libass retombe sur Arial.
- Captures d'écran de test : utiliser `PrintWindow` sur la fenêtre de l'app, pas une capture de l'écran (qui peut
  montrer ce que l'utilisateur fait par-dessus).
- **Ne pas utiliser la caisse `dirs`** : avec elle, l'exe graphique plantait avant `main` (violation d'accès dans un
  module inconnu) ; les dossiers Documents / Musique sont lus dans le registre (`library::user_folder`).
- Données utilisateur dans `Documents\MagicMus` ; outils et modèles dans `%LOCALAPPDATA%\Karaoke` (volumineux).
- Diagnostic de plantage : `erreurs.log` (panics) ; avec l'option `demo`, un gestionnaire d'exceptions Windows y
  ajoute les violations d'accès, et `traces.log` liste les étapes du démarrage.

## État (2026-11-09)

- Vérifié : séparation IA (comparée à une référence Python, 31 dB), export, lecteur, bascule voix originale
  (pistes alignées à l'échantillon), recherche LRCLIB + alignement sur LRC.
- **Non vérifié par Claude** : l'exécution de whisper.cpp (binaire téléchargé, refusé par le contrôle de sécurité).
  L'utilisateur a jugé la reconnaissance seule mauvaise, d'où LRCLIB et l'éditeur de paroles.
- Piste en attente : version web sur un serveur Proxmox (i5-7500, 4 cœurs, ~5 Go libres) avec lecteur dans le
  navigateur (Signalsmith en WASM).
