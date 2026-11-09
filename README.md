# MagicMus

Le karaoké qui fait disparaître la voix : application Windows (Rust) qui télécharge une vidéo YouTube, affiche les
paroles synchronisées, supprime la voix et lit la version instrumentale.

## Utilisation

Double-cliquez sur **`dist\MagicMus.exe`**. Collez l'adresse YouTube (ou choisissez un fichier), puis cliquez sur
**Créer le karaoké**.

## Bibliothèque, historique et données

Tout est enregistré dans **`Documents\MagicMus`** :

| Élément | Contenu |
|---|---|
| `Chansons\` | un dossier par chanson (vidéo d'origine, karaoké, paroles) |
| `bibliotheque.json` | dossiers et chansons de la bibliothèque |
| `historique.json` | chansons créées et écoutées |
| `reglages.json` | méthode, langue, couleur et taille des paroles… |
| `erreurs.log` | détails d'un éventuel plantage |

- **Historique** (onglet du panneau gauche) : chansons regroupées par jour ; clic pour rouvrir, étoile pour ajouter à
  la bibliothèque, clic droit pour plus d'options.
- **Bibliothèque** : dossiers et sous-dossiers ; glisser une chanson (ou un dossier) sur un dossier pour la ranger,
  clic droit pour renommer, déplacer, supprimer un dossier (les chansons sont conservées) ; recherche par titre.
  Le bouton « Ajouter à la bibliothèque » de l'en-tête du lecteur range la chanson ouverte.
- Les karaokés de l'ancien emplacement (`Musique\Karaoke`) sont détectés au démarrage : un bandeau propose de les
  déplacer dans `Documents\MagicMus\Chansons`.
- La variable d'environnement `MAGICMUS_DATA` permet d'utiliser un autre dossier (version portable).

Les outils et modèles d'IA (≈ 1 Go) restent dans `%LOCALAPPDATA%\Karaoke` : ils font partie de l'application, et
Documents est souvent synchronisé avec OneDrive.

## Design

Interface d'après la maquette « MagicMus design modern » (Claude Design) : tons crème et rose, boutons noir prune,
titres en DM Serif Display, texte en Nunito (polices OFL, `assets/fonts`), logo `assets/magicmus-logo.png`.
Couleurs et composants dessinés à la main dans `src/theme.rs`. Dans les options : couleur des mots chantés (rose, or,
crème, lavande) et taille des paroles.

**Aucune installation n'est nécessaire.** Au premier usage, l'application télécharge elle-même ce dont elle a besoin
dans `%LOCALAPPDATA%\Karaoke` (environ 300 Mo, une seule fois) :

| Élément | Rôle |
|---|---|
| yt-dlp + Deno | téléchargement des vidéos YouTube (Deno résout les protections JavaScript de YouTube) |
| ffmpeg / ffprobe / ffplay | décodage, encodage et lecture |
| ONNX Runtime (Microsoft) | moteur d'exécution du réseau de neurones |
| Modèle UVR-MDX-NET Inst HQ 3 | réseau de neurones de séparation voix / musique |

Si un outil est déjà installé sur la machine (dans le PATH), il est utilisé tel quel. Si une vidéo ne se télécharge
plus, l'application met automatiquement yt-dlp à jour.

Version hors ligne : placez les outils (`yt-dlp.exe`, `deno.exe`, `ffmpeg.exe`, `ffprobe.exe`, `ffplay.exe` et leurs DLL,
`onnxruntime.dll`) dans un dossier `tools` à côté de `Karaoke.exe`, et le modèle dans `models\`.

## Lecteur intégré

Le karaoké se lit dans la fenêtre :

- **Lecture** : bouton Lecture/Pause (ou barre d'espace), barre de progression, ←/→ pour reculer ou avancer de 5 s, clic sur l'image pour la pause.
- **Comparer avec l'original** : le bouton « 🎤 Voix » (ou la touche V) bascule à tout moment entre le karaoké et la
  version originale avec la voix, sans interruption (fondu de 40 ms). La position, la tonalité et la vitesse sont
  conservées. L'original est chargé en arrière-plan à l'ouverture.
- **Paroles synchronisées** : chaque mot s'allume au moment où il est chanté. Le texte vient, par ordre de priorité :
  1. des paroles collées par l'utilisateur (texte ou LRC) ;
  2. de la base libre en ligne [LRCLIB](https://lrclib.net) (paroles officielles, horodatées vers par vers) ;
  3. à défaut, de la reconnaissance vocale.

  Le minutage mot à mot vient de la voix isolée, transcrite par Whisper (whisper.cpp, modèle large-v3-turbo,
  ≈ 550 Mo au premier usage) : les mots exacts sont alignés sur les mots entendus (`src/align.rs`).
- **Corriger les paroles** : bouton « Corriger… » dans le lecteur ; le texte corrigé est recalé automatiquement.
  Le réglage **Décalage** avance ou retarde l'ensemble des paroles. **Resynchroniser** recale en une seconde les
  paroles d'un morceau déjà créé sur la voix entendue (sans relancer la reconnaissance vocale).
- **Tonalité** (−12 à +12 demi-tons) et **vitesse** (50 à 150 %) se règlent pendant la lecture, sans que l'une modifie
  l'autre (Signalsmith Stretch, intégré dans `vendor/signalsmith`).
- **Enregistrer…** : crée un fichier avec la tonalité et la vitesse actuelles, et les paroles karaoké incrustées dans la
  vidéo si la case est cochée.

Un karaoké déjà créé se rouvre avec « Ouvrir un karaoké déjà créé… », par glisser-déposer sur la fenêtre ou en
passant le fichier en argument à `Karaoke.exe`.

## Méthodes

- **Qualité (IA, par défaut)** : le réseau de neurones MDX-Net prédit le spectrogramme de l'accompagnement. Il tourne en
  Rust via ONNX Runtime, sans Python. Il faut compter environ une minute par chanson sur le processeur.
- **Rapide** : masque spectral stéréo. Pour chaque fréquence, l'outil mesure la ressemblance `2·Re(L·R̄)/(|L|²+|R|²)`
  entre les deux canaux et atténue ce qui est mixé au centre, là où se trouve la voix, dans la bande 150–8000 Hz.
  Cette méthode est instantanée, mais moins propre, et inutilisable sur un audio mono.

## Ligne de commande

```bash
dist\karaoke-cli.exe "https://www.youtube.com/watch?v=XXXXXXXXXXX"
```

Options : `-m ia|spectral`, `-a` (audio seulement), `-s 0.8` (intensité, méthode rapide), `-o DOSSIER`,
`--no-play`, `-f` (tout refaire). Voir `--help`.

## Compilation

```bash
cargo build --release
```

On obtient `target/release/karaoke.exe` (interface graphique) et `target/release/karaoke-cli.exe`.

## Organisation du code

| Fichier | Rôle |
|---|---|
| `src/main.rs` | interface graphique (egui) : accueil, traitement, lecteur, correction des paroles |
| `src/theme.rs` | couleurs, polices, logo et composants du design MagicMus |
| `src/ui_library.rs` | onglets Bibliothèque et Historique, réglages enregistrés |
| `src/library.rs` | données de la bibliothèque et de l'historique (`Documents\MagicMus`) |
| `src/player.rs` | lecteur intégré : son (cpal), image (ffmpeg), paroles |
| `src/lyrics.rs` | transcription Whisper, lignes, sous-titres ASS karaoké |
| `src/lrclib.rs` | recherche des paroles officielles (LRCLIB) |
| `src/align.rs` | alignement des paroles exactes sur les mots reconnus |
| `src/stretch.rs` | tonalité / vitesse (liaison vers Signalsmith Stretch) |
| `src/export.rs` | enregistrement avec tonalité, vitesse et paroles incrustées |
| `src/bin/karaoke-cli.rs` | ligne de commande |
| `src/pipeline.rs` | enchaînement des étapes |
| `src/setup.rs` | téléchargement automatique des outils et du modèle |
| `src/mdx.rs` | séparation par IA (STFT, inférence ONNX, STFT inverse) |
| `src/dsp.rs` | méthode rapide (masque spectral) |
| `src/download.rs` | yt-dlp |
| `src/media.rs` | ffmpeg, ffplay |
| `src/tools.rs` | lancement des outils externes sans fenêtre de console |

## Tests

```bash
cargo test
```
