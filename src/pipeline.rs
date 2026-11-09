//! Enchaînement complet : source → suppression de la voix → fichier karaoké.
//!
//!   0. setup    : téléchargement automatique des outils et du modèle si besoin
//!   1. yt-dlp   : téléchargement de la vidéo (ou de l'audio seul)
//!   2. ffmpeg   : décodage de la piste audio en PCM stéréo f32
//!   3. Rust     : suppression de la voix — masque spectral (`dsp`)
//!                 ou réseau de neurones MDX-Net via ONNX Runtime (`mdx`)
//!   4. ffmpeg   : ré-encodage de l'audio et remultiplexage avec la vidéo

use crate::lyrics::{self, Lyrics};
use crate::{align, download, dsp, lrclib, mdx, media, setup, tools};

/// Métadonnées utiles à la recherche des paroles.
#[derive(Default)]
struct Meta {
    artist: Option<String>,
    track: Option<String>,
}
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

#[derive(Copy, Clone, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Method {
    /// Traitement spectral intégré (quelques secondes) : retire ce qui est
    /// mixé au centre de l'image stéréo, là où se trouve presque toujours la voix.
    Spectral,
    /// Séparation par réseau de neurones (MDX-Net, intégré) : bien meilleure
    /// qualité, environ une minute par chanson.
    Ia,
}

#[derive(Clone, Debug)]
pub struct Options {
    /// URL YouTube ou chemin d'un fichier local.
    pub input: String,
    pub method: Method,
    pub strength: f32,
    pub selectivity: f32,
    pub low_hz: f32,
    pub high_hz: f32,
    pub audio_only: bool,
    pub out_dir: PathBuf,
    pub force: bool,
    /// Transcrire les paroles (synchronisées mot à mot) avant de retirer la voix.
    pub lyrics: bool,
    /// Langue des paroles : code ISO (« fr », « en »…) ou « auto ».
    pub language: String,
    /// Chercher les paroles exactes dans la base en ligne LRCLIB.
    pub lyrics_online: bool,
    /// Paroles exactes saisies par l'utilisateur (prioritaires).
    pub lyrics_text: String,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            input: String::new(),
            method: Method::Ia,
            strength: 1.0,
            selectivity: 4.0,
            low_hz: 150.0,
            high_hz: 8000.0,
            audio_only: false,
            out_dir: PathBuf::from("karaoke_out"),
            force: false,
            lyrics: true,
            language: "auto".into(),
            lyrics_online: true,
            lyrics_text: String::new(),
        }
    }
}

/// Ce qui se passe pendant le traitement.
#[derive(Clone, Debug)]
pub enum Event {
    /// Nouvelle étape (remet la progression à « indéterminée »).
    Stage(String),
    /// Progression de l'étape en cours, de 0.0 à 1.0.
    Progress(f32),
    /// Information détaillée.
    Log(String),
    /// Avertissement à montrer à l'utilisateur.
    Warning(String),
}

pub struct Outcome {
    /// Fichier karaoké (vidéo ou audio sans voix).
    pub output: PathBuf,
    pub title: String,
    pub has_video: bool,
    /// Paroles synchronisées, si demandées et reconnues.
    pub lyrics: Option<Lyrics>,
    /// Fichier d'origine (avec la voix), pour la comparaison pendant la lecture.
    pub source: Option<PathBuf>,
}

/// Fichier mémorisant le chemin de la source (utile pour les fichiers locaux).
const SOURCE_FILE: &str = "source.txt";

impl Outcome {
    /// Recharge un karaoké déjà produit (paroles comprises si elles existent).
    pub fn open(output: &Path) -> Self {
        let dir = output.parent().unwrap_or(Path::new("."));
        let title = std::fs::read_to_string(dir.join("titre.txt"))
            .ok()
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| output.file_stem().unwrap_or_default().to_string_lossy().into_owned());
        Self {
            output: output.to_path_buf(),
            title,
            has_video: media::has_video(output),
            lyrics: Lyrics::load(&dir.join(LYRICS_FILE)).ok(),
            source: std::fs::read_to_string(dir.join(SOURCE_FILE))
                .ok()
                .map(|s| PathBuf::from(s.trim()))
                .filter(|p| p.is_file())
                .or_else(|| download::find_source(dir)),
        }
    }
}

pub const LYRICS_FILE: &str = "paroles.json";

/// Adapte les messages de `setup` (message, progression) en événements.
fn setup_reporter(emit: &mut dyn FnMut(Event)) -> impl FnMut(&str, Option<f32>) + '_ {
    let mut last = String::new();
    move |msg: &str, p: Option<f32>| {
        if msg != last {
            last = msg.to_string();
            emit(Event::Stage(last.clone()));
        }
        if let Some(p) = p {
            emit(Event::Progress(p));
        }
    }
}

pub fn run(o: &Options, emit: &mut dyn FnMut(Event)) -> Result<Outcome> {
    if !(0.0..=1.0).contains(&o.strength) {
        bail!("l'intensité doit être comprise entre 0.0 et 1.0");
    }
    if o.low_hz >= o.high_hz {
        bail!("la fréquence basse doit être inférieure à la fréquence haute");
    }
    let input = clean_input(&o.input);
    if input.is_empty() {
        bail!("indiquez une adresse YouTube ou un fichier");
    }
    let local = Path::new(input);
    let is_local = local.is_file();

    // 0. Outils (téléchargés automatiquement au premier lancement)
    emit(Event::Stage("Vérification des outils".into()));
    setup::ensure_tools(!is_local, &mut setup_reporter(emit))?;

    // 1. Source : fichier local ou téléchargement YouTube
    let (source, work_dir, title, meta) = if is_local {
        let stem = local
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "local".into());
        let dir = o.out_dir.join(sanitize(&stem));
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("impossible de créer {}", dir.display()))?;
        (local.to_path_buf(), dir, stem, Meta::default())
    } else {
        emit(Event::Stage("Récupération des informations de la vidéo".into()));
        let info = match download::fetch_info(input) {
            Ok(info) => info,
            Err(first) => {
                // YouTube change souvent : une version récente de yt-dlp règle
                // la plupart des erreurs.
                emit(Event::Stage("Mise à jour de yt-dlp".into()));
                if download::self_update().is_err() {
                    return Err(first);
                }
                download::fetch_info(input)?
            }
        };
        emit(Event::Log(format!("« {} » ({})", info.title, info.id)));
        let dir = o.out_dir.join(sanitize(&info.id));
        let source = match download::find_source(&dir).filter(|_| !o.force) {
            Some(p) => {
                emit(Event::Log(format!("Déjà téléchargé : {}", p.display())));
                p
            }
            None => {
                emit(Event::Stage("Téléchargement de la vidéo".into()));
                download::download(input, &dir, o.audio_only, &mut |line| {
                    match line.starts_with("[download]").then(|| tools::parse_percent(line)) {
                        Some(Some(p)) => emit(Event::Progress(p)),
                        _ => emit(Event::Log(line.to_string())),
                    }
                })?
            }
        };
        (source, dir, info.title, Meta { artist: info.artist, track: info.track })
    };

    let with_video = !o.audio_only && media::has_video(&source);
    let suffix = match o.method {
        Method::Spectral => "rapide",
        Method::Ia => "ia",
    };
    let ext = if with_video { "mp4" } else { "m4a" };
    let output = work_dir.join(format!("karaoke_{suffix}.{ext}"));

    let _ = std::fs::write(work_dir.join("titre.txt"), &title);
    if let Ok(abs) = std::path::absolute(&source) {
        let _ = std::fs::write(work_dir.join(SOURCE_FILE), abs.to_string_lossy().as_bytes());
    }
    let lyrics_path = work_dir.join(LYRICS_FILE);
    let whisper_json = work_dir.join(lyrics::WHISPER_JSON);
    let user_text = !o.lyrics_text.trim().is_empty();
    let need_lyrics = o.lyrics && (o.force || user_text || !lyrics_path.is_file());
    // La reconnaissance (lente) n'est refaite que si son résultat brut manque.
    let need_recognition = need_lyrics && (o.force || !whisper_json.is_file());

    // 2-4. Transcription des paroles et suppression de la voix
    if output.is_file() && !o.force && !need_recognition {
        emit(Event::Log(format!("Version sans voix déjà générée : {}", output.display())));
    } else {
        // Préparation de la reconnaissance des paroles (téléchargée au premier usage).
        let whisper = if need_recognition {
            let res = setup::ensure_whisper(&mut setup_reporter(emit));
            match res {
                Ok(w) => Some(w),
                Err(e) => {
                    emit(Event::Warning(format!("Paroles indisponibles : {e:#}")));
                    None
                }
            }
        } else {
            None
        };
        let model = match o.method {
            Method::Ia => {
                let runtime = setup::ensure_onnxruntime(&mut setup_reporter(emit))?;
                let model = setup::ensure_model(&mut setup_reporter(emit))?;
                Some((runtime, model))
            }
            Method::Spectral => None,
        };

        emit(Event::Stage("Décodage de l'audio".into()));
        let pcm = media::decode_pcm(&source)?;
        if pcm.is_empty() {
            bail!("aucune piste audio trouvée dans {}", source.display());
        }
        let seconds = pcm.len() as f32 / 2.0 / media::SAMPLE_RATE as f32;
        emit(Event::Log(format!("{seconds:.0} s d'audio")));

        let processed = match model {
            Some((runtime, model)) => {
                emit(Event::Stage("Séparation voix / musique par IA".into()));
                mdx::separate(&runtime, &model, &pcm, &mut |p| emit(Event::Progress(p)))
                    .context("échec de la séparation par IA")?
            }
            None => spectral(o, &pcm, emit),
        };

        // Paroles : transcription de la voix isolée (mélange − accompagnement).
        if let Some(whisper) = whisper {
            emit(Event::Stage("Reconnaissance des paroles".into()));
            let vocals: Vec<f32> = pcm.iter().zip(&processed).map(|(m, i)| m - i).collect();
            match lyrics::transcribe(&whisper, &vocals, media::SAMPLE_RATE, &work_dir, &o.language, &mut |p| {
                emit(Event::Progress(p))
            }) {
                Ok((lang, words)) => {
                    emit(Event::Log(format!("Reconnaissance : {} mots (langue : {lang})", words.len())))
                }
                Err(e) => emit(Event::Warning(format!("Paroles non reconnues : {e:#}"))),
            }
        }

        if !output.is_file() || o.force {
            emit(Event::Stage("Encodage".into()));
            media::encode_pcm(&source, &processed, &output, with_video)?;
        }
        emit(Event::Log(format!("Fichier karaoké : {}", output.display())));
    }

    // 5. Paroles finales : texte exact (saisi ou trouvé en ligne) calé sur la reconnaissance.
    if need_lyrics {
        emit(Event::Stage("Synchronisation des paroles".into()));
        let req = LyricsRequest {
            work_dir: &work_dir,
            title: &title,
            artist: meta.artist.as_deref(),
            track: meta.track.as_deref(),
            duration: media::duration(&output).unwrap_or(0.0),
            language: &o.language,
            user_text: user_text.then_some(o.lyrics_text.as_str()),
            online: o.lyrics_online,
        };
        if let Some(ly) = build_lyrics(&req, emit) {
            ly.save(&lyrics_path)?;
        }
    }

    let mut outcome = Outcome::open(&output);
    outcome.title = title;
    if !o.lyrics {
        outcome.lyrics = None;
    }
    Ok(outcome)
}

/// Données nécessaires pour construire (ou reconstruire) les paroles synchronisées.
pub struct LyricsRequest<'a> {
    /// Dossier de la chanson (contient `whisper.json`).
    pub work_dir: &'a Path,
    pub title: &'a str,
    pub artist: Option<&'a str>,
    pub track: Option<&'a str>,
    /// Durée du morceau (s).
    pub duration: f64,
    pub language: &'a str,
    /// Texte exact fourni par l'utilisateur (prioritaire sur la recherche en ligne).
    pub user_text: Option<&'a str>,
    /// Chercher les paroles exactes dans LRCLIB.
    pub online: bool,
}

/// Paroles finales : texte exact (saisi, sinon trouvé en ligne) aligné sur la voix
/// reconnue par Whisper ; à défaut, la reconnaissance seule. Ne relance pas Whisper :
/// utilise `whisper.json` s'il existe.
pub fn build_lyrics(r: &LyricsRequest, emit: &mut dyn FnMut(Event)) -> Option<Lyrics> {
    let (language, recognized) =
        lyrics::read_whisper(&r.work_dir.join(lyrics::WHISPER_JSON), r.language).unwrap_or_default();
    let (text, origin, short) = if let Some(t) = r.user_text {
        (Some(lyrics::parse_text(t)), "paroles saisies".to_string(), "texte saisi")
    } else if r.online {
        match lrclib::find(r.title, r.artist, r.track, r.duration) {
            Ok(Some(m)) => {
                let synced = matches!(m.lyrics, lrclib::Found::Synced(_));
                let lines = match m.lyrics {
                    lrclib::Found::Synced(l) => {
                        l.into_iter().map(|(t, s)| align::TextLine { start: Some(t), text: s }).collect()
                    }
                    lrclib::Found::Plain(l) => l.into_iter().map(|s| align::TextLine { start: None, text: s }).collect(),
                };
                let kind = if synced { "synchronisées" } else { "non synchronisées" };
                (Some(lines), format!("LRCLIB, {kind} : {}", m.description), "LRCLIB")
            }
            Ok(None) => (None, "reconnaissance vocale (introuvables en ligne)".into(), "reconnaissance vocale"),
            Err(e) => {
                emit(Event::Log(format!("Recherche en ligne impossible : {e:#}")));
                (None, "reconnaissance vocale".into(), "reconnaissance vocale")
            }
        }
    } else {
        (None, "reconnaissance vocale".into(), "reconnaissance vocale")
    };

    let text_synced = text.as_ref().is_some_and(|t| t.iter().any(|l| l.start.is_some()));
    let result = match text {
        // Texte exact : horodaté (LRC) ou calé sur la reconnaissance.
        Some(t) if text_synced || !recognized.is_empty() => Some(Lyrics {
            language: language.clone(),
            lines: align::align(&t, &recognized, r.duration),
            offset: 0.0,
            source: short.into(),
        }),
        Some(_) => {
            emit(Event::Warning(
                "Paroles trouvées sans minutage et reconnaissance vocale indisponible : impossible de les synchroniser."
                    .into(),
            ));
            None
        }
        None if !recognized.is_empty() => Some(Lyrics {
            language: language.clone(),
            lines: lyrics::group_lines(recognized),
            offset: 0.0,
            source: "reconnaissance vocale".into(),
        }),
        None => None,
    };
    match result.filter(|l| !l.lines.is_empty()) {
        Some(ly) => {
            emit(Event::Log(format!("Paroles : {} lignes, {} mots — source : {origin}", ly.lines.len(), ly.word_count())));
            Some(ly)
        }
        None => {
            emit(Event::Warning("Aucune parole disponible pour ce morceau.".into()));
            None
        }
    }
}

fn spectral(o: &Options, pcm: &[f32], emit: &mut dyn FnMut(Event)) -> Vec<f32> {
    if dsp::stereo_width(pcm) < 1e-4 {
        emit(Event::Warning(
            "L'audio est (quasi) mono : la méthode rapide ne peut pas distinguer la voix \
             et atténue tout. Utilisez la méthode IA."
                .into(),
        ));
    }
    emit(Event::Stage("Suppression de la voix".into()));
    let params = dsp::Params {
        sample_rate: media::SAMPLE_RATE,
        strength: o.strength,
        selectivity: o.selectivity,
        low_hz: o.low_hz,
        high_hz: o.high_hz,
    };
    let mut last = -1i32;
    dsp::remove_vocals(pcm, &params, |p| {
        let pct = (p * 100.0) as i32;
        if pct != last {
            last = pct;
            emit(Event::Progress(p));
        }
    })
}

/// Nettoie la saisie : espaces, guillemets, et adresse collée à la suite d'une autre
/// (« https://…https://www.youtube.com/watch?v=… » → on garde la dernière).
pub fn clean_input(raw: &str) -> &str {
    let s = raw.trim().trim_matches('"').trim();
    if Path::new(s).exists() {
        return s;
    }
    match s.rfind("https://").or_else(|| s.rfind("http://")) {
        Some(i) => s[i..].split_whitespace().next().unwrap_or(""),
        None => s,
    }
}

/// Nom de dossier sûr sur tous les systèmes de fichiers.
pub fn sanitize(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    if s.is_empty() { "video".into() } else { s }
}

#[cfg(test)]
mod tests {
    use super::clean_input;

    #[test]
    fn nettoyage_saisie() {
        let u = "https://www.youtube.com/watch?v=abc&list=RDabc";
        assert_eq!(clean_input(u), u);
        assert_eq!(clean_input(&format!("  \"{u}\" ")), u);
        assert_eq!(clean_input(&format!("https://www.youtube.com/{u}")), u);
        assert_eq!(clean_input(&format!("https://youtu.be/xyz{u}")), u);
        assert_eq!(clean_input("dQw4w9WgXcQ"), "dQw4w9WgXcQ");
    }
}
