//! Version ligne de commande : `karaoke-cli <URL> [options]`.

use clap::Parser;
use karaoke::pipeline::{self, Event, Method, Options};
use karaoke::{media, tools};
use std::io::Write;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "karaoke-cli",
    version,
    about = "Télécharge une vidéo YouTube, supprime la voix et lit la version karaoké"
)]
struct Cli {
    /// URL YouTube (ou chemin d'un fichier audio/vidéo local)
    input: String,

    /// Méthode de suppression de la voix
    #[arg(short, long, value_enum, default_value_t = Method::Ia)]
    method: Method,

    /// Intensité de la suppression, de 0.0 (aucune) à 1.0 (maximale) — méthode spectrale
    #[arg(short, long, default_value_t = 1.0)]
    strength: f32,

    /// Sélectivité : plus la valeur est haute, plus seul ce qui est parfaitement
    /// centré est retiré (préserve mieux les instruments) — méthode spectrale
    #[arg(long, default_value_t = 4.0)]
    selectivity: f32,

    /// Fréquence basse de la bande traitée en Hz — méthode spectrale
    #[arg(long, default_value_t = 150.0)]
    low_hz: f32,

    /// Fréquence haute de la bande traitée en Hz — méthode spectrale
    #[arg(long, default_value_t = 8000.0)]
    high_hz: f32,

    /// Ne télécharger et ne traiter que l'audio
    #[arg(short, long)]
    audio_only: bool,

    /// Dossier de travail et de sortie
    #[arg(short, long, default_value = "karaoke_out")]
    out_dir: PathBuf,

    /// Ne pas lancer la lecture à la fin
    #[arg(long)]
    no_play: bool,

    /// Refaire le téléchargement et le traitement même si les fichiers existent
    #[arg(short, long)]
    force: bool,

    /// Ne pas transcrire les paroles
    #[arg(long)]
    no_lyrics: bool,

    /// Langue des paroles (fr, en, es…) ou « auto »
    #[arg(short, long, default_value = "auto")]
    language: String,

    /// Ne pas chercher les paroles exactes en ligne (LRCLIB)
    #[arg(long)]
    no_online: bool,

    /// Fichier texte ou LRC contenant les paroles exactes
    #[arg(long)]
    lyrics_file: Option<PathBuf>,
}

fn main() {
    tools::setup_path();
    let cli = Cli::parse();
    let opts = Options {
        input: cli.input,
        method: cli.method,
        strength: cli.strength,
        selectivity: cli.selectivity,
        low_hz: cli.low_hz,
        high_hz: cli.high_hz,
        audio_only: cli.audio_only,
        out_dir: cli.out_dir,
        force: cli.force,
        lyrics: !cli.no_lyrics,
        language: cli.language,
        lyrics_online: !cli.no_online,
        lyrics_text: match &cli.lyrics_file {
            Some(f) => match std::fs::read_to_string(f) {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("Erreur : lecture de {} : {e}", f.display());
                    std::process::exit(1);
                }
            },
            None => String::new(),
        },
    };

    let mut in_progress = false;
    let mut emit = |e: Event| {
        if in_progress && !matches!(e, Event::Progress(_)) {
            println!();
            in_progress = false;
        }
        match e {
            Event::Stage(s) => println!("» {s}…"),
            Event::Progress(p) => {
                print!("\r   {:3.0} %", p * 100.0);
                let _ = std::io::stdout().flush();
                in_progress = true;
            }
            Event::Log(l) => println!("   {l}"),
            Event::Warning(w) => eprintln!("⚠  {w}"),
        }
    };

    match pipeline::run(&opts, &mut emit) {
        Ok(out) => {
            println!("✔ {}", out.output.display());
            if !cli.no_play {
                println!("▶  Lecture — Espace : pause · ←/→ : ±10 s · q ou Échap : quitter");
                if let Err(e) = media::play(&out.output, &format!("Karaoké — {}", out.title), true) {
                    eprintln!("Erreur : {e:#}");
                }
            }
        }
        Err(e) => {
            eprintln!("\nErreur : {e:#}");
            std::process::exit(1);
        }
    }
}
