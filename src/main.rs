//! MagicMus — application graphique : télécharge une vidéo YouTube, transcrit les
//! paroles, supprime la voix et lit le karaoké dans la fenêtre (tonalité et vitesse
//! réglables). Design : maquette « MagicMus design modern » (Claude Design).

// Pas de fenêtre de console en version release.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod theme;
mod ui_library;

use ui_library::{LibUi, Settings, Store, Tab};

use eframe::egui::{
    self, text::LayoutJob, Align, Align2, Color32, CornerRadius, Frame, Layout, Margin, Pos2, Rect, Sense, Stroke,
    TextFormat, TextureHandle, Vec2,
};
use karaoke::export::{self, ExportSettings};
use karaoke::pipeline::{self, Event, Method, Options, Outcome};
use karaoke::player::{self, LyricsStyle, Player};
use karaoke::{media, tools};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use theme::{self as th, W};

const LANGUAGES: [(&str, &str); 8] = [
    ("auto", "Détection automatique"),
    ("fr", "Français"),
    ("en", "Anglais"),
    ("es", "Espagnol"),
    ("it", "Italien"),
    ("de", "Allemand"),
    ("pt", "Portugais"),
    ("ja", "Japonais"),
];

/// Étapes affichées pendant la création, dans l'ordre du traitement.
const STEPS: [&str; 7] = [
    "Préparation des outils",
    "Téléchargement de la vidéo",
    "Décodage de l'audio",
    "Séparation de la voix",
    "Reconnaissance des paroles",
    "Encodage du karaoké",
    "Synchronisation des paroles",
];
const LYRICS_STEPS: [usize; 2] = [4, 6];

/// Étape correspondant à un message du traitement (`None` : étape inchangée,
/// par exemple le téléchargement d'un outil au premier usage).
fn step_of(stage: &str) -> Option<usize> {
    let s = stage.to_lowercase();
    let starts = |p: &str| s.starts_with(p);
    if starts("vérification des outils") {
        Some(0)
    } else if s.contains("informations de la vidéo") || starts("téléchargement de la vidéo") || starts("mise à jour de yt-dlp") {
        Some(1)
    } else if starts("décodage") {
        Some(2)
    } else if starts("séparation") || starts("suppression de la voix") {
        Some(3)
    } else if starts("reconnaissance des paroles") {
        Some(4)
    } else if starts("encodage") {
        Some(5)
    } else if starts("synchronisation") {
        Some(6)
    } else {
        None
    }
}

/// Enregistre les plantages dans `Documents\MagicMus\erreurs.log` (l'application n'a
/// pas de console : sans cela, le message serait perdu).
fn install_crash_log() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let dir = karaoke::library::data_dir();
        let _ = std::fs::create_dir_all(&dir);
        let bt = std::backtrace::Backtrace::force_capture();
        let msg = format!("[{}] {info}\n{bt}\n", karaoke::library::now());
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("erreurs.log")) {
            use std::io::Write;
            let _ = f.write_all(msg.as_bytes());
        }
        default(info);
    }));
}

/// Diagnostic (option « demo ») : enregistre l'adresse et la pile d'appels de toute
/// violation d'accès dans `erreurs.log`.
#[cfg(feature = "demo")]
fn install_exception_log() {
    use windows_sys::Win32::Foundation::EXCEPTION_ACCESS_VIOLATION;
    use windows_sys::Win32::System::Diagnostics::Debug::{AddVectoredExceptionHandler, EXCEPTION_POINTERS};
    unsafe extern "system" fn handler(info: *mut EXCEPTION_POINTERS) -> i32 {
        unsafe {
            let rec = &*(*info).ExceptionRecord;
            if rec.ExceptionCode == EXCEPTION_ACCESS_VIOLATION {
                let path = karaoke::library::data_dir().join("erreurs.log");
                let head = format!(
                    "violation d'accès à {:?} (opération {}, adresse {:#x})\n",
                    rec.ExceptionAddress, rec.ExceptionInformation[0], rec.ExceptionInformation[1]
                );
                let _ = std::fs::write(&path, &head); // d'abord l'essentiel
                let bt = std::backtrace::Backtrace::force_capture();
                let _ = std::fs::write(&path, format!("{head}{bt}\n"));
            }
        }
        0 // EXCEPTION_CONTINUE_SEARCH
    }
    unsafe {
        AddVectoredExceptionHandler(1, Some(handler));
    }
}

/// Trace de démarrage (option « demo ») dans `traces.log`.
#[cfg(feature = "demo")]
fn trace(step: &str) {
    use std::io::Write;
    let path = karaoke::library::data_dir().join("traces.log");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{step}");
    }
}
#[cfg(not(feature = "demo"))]
fn trace(_: &str) {}

fn main() -> eframe::Result {
    #[cfg(feature = "demo")]
    install_exception_log();
    trace("main");
    install_crash_log();
    tools::setup_path();
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("MagicMus")
        .with_inner_size([1280.0, 820.0])
        .with_min_inner_size([1200.0, 700.0]);
    if let Some(icon) = th::window_icon() {
        viewport = viewport.with_icon(std::sync::Arc::new(icon));
    }
    let options = eframe::NativeOptions { viewport, ..Default::default() };
    // Un fichier passé en argument (« Ouvrir avec… ») est ouvert directement.
    let arg = std::env::args_os().nth(1).map(PathBuf::from);
    trace("avant run_native");
    eframe::run_native(
        "MagicMus",
        options,
        Box::new(move |cc| {
            trace("création de l'application");
            th::install(&cc.egui_ctx);
            trace("polices et thème installés");
            let mut app = App::new(th::logo(&cc.egui_ctx));
            trace("application créée");
            if let Some(path) = arg.filter(|p| p.is_file()) {
                app.open_path(path);
            }
            #[cfg(feature = "demo")]
            app.demo(std::env::var("MAGICMUS_DEMO").ok().as_deref(), &cc.egui_ctx);
            Ok(Box::new(app))
        }),
    )
}

enum Msg {
    Event(Event),
    Finished(Result<Outcome, String>),
}

enum ResyncMsg {
    Progress(String, Option<f32>),
    Done(Option<karaoke::lyrics::Lyrics>, Vec<String>),
}

enum ExportMsg {
    Progress(String, Option<f32>),
    Done(Result<PathBuf, String>),
}

/// Avancement d'une tâche de fond.
#[derive(Default)]
struct Task {
    stage: String,
    progress: Option<f32>,
    step: usize,
}

struct App {
    logo: TextureHandle,
    opts: Options,
    auto_play: bool,
    rx: Option<Receiver<Msg>>,
    task: Task,
    log: Vec<String>,
    warnings: Vec<String>,
    error: Option<String>,

    // Panneau de gauche
    paste_open: bool,
    options_open: bool,
    journal_open: bool,

    // Lecteur
    player: Option<Player>,
    has_video: bool,
    show_lyrics: bool,
    seek_drag: Option<f64>,
    highlight: usize,
    lyrics_size: f32,

    // Paroles et enregistrement
    burn_lyrics: bool,
    lyrics_editor: Option<String>,
    export_rx: Option<Receiver<ExportMsg>>,
    export_task: Task,
    export_result: Option<Result<PathBuf, String>>,
    /// Resynchronisation des paroles en cours, et son dernier résultat (message, réussite, instant).
    resync_rx: Option<Receiver<ResyncMsg>>,
    /// Étape et progression de la resynchronisation en cours.
    resync_status: Option<(String, Option<f32>)>,
    resync_msg: Option<(String, bool, std::time::Instant)>,

    // Bibliothèque, historique et réglages (Documents\MagicMus)
    store: Store,
    settings: Settings,
    lib_ui: LibUi,
}

impl App {
    fn new(logo: TextureHandle) -> Self {
        // Bibliothèque, historique, réglages et chansons : Documents\MagicMus.
        let (store, settings) = Store::open();
        let out_dir = settings.out_dir.clone().unwrap_or_else(karaoke::library::songs_dir);
        let opts = Options {
            out_dir,
            method: settings.method(),
            language: settings.language.clone(),
            lyrics: settings.lyrics,
            lyrics_online: settings.lyrics_online,
            ..Options::default()
        };
        let mut app = Self {
            logo,
            opts,
            auto_play: settings.auto_play,
            rx: None,
            task: Task::default(),
            log: Vec::new(),
            warnings: Vec::new(),
            error: None,
            paste_open: false,
            options_open: false,
            journal_open: false,
            player: None,
            has_video: false,
            show_lyrics: true,
            seek_drag: None,
            highlight: settings.highlight.min(th::HIGHLIGHTS.len() - 1),
            lyrics_size: settings.lyrics_size.clamp(24.0, 52.0),
            burn_lyrics: settings.burn_lyrics,
            lyrics_editor: None,
            export_rx: None,
            export_task: Task::default(),
            export_result: None,
            resync_rx: None,
            resync_status: None,
            resync_msg: None,
            store,
            settings: settings.clone(),
            lib_ui: LibUi::default(),
        };
        app.detect_old_songs(&settings);
        // Dossiers de la bibliothèque dépliés au démarrage.
        app.lib_ui.open = app.store.library.folders.iter().map(|f| f.id).collect();
        app
    }

    /// Recopie les réglages courants et les enregistre s'ils ont changé.
    fn sync_settings(&mut self) {
        let default_dir = karaoke::library::songs_dir();
        let s = &mut self.settings;
        s.highlight = self.highlight;
        s.lyrics_size = self.lyrics_size;
        s.quality = self.opts.method == Method::Ia;
        s.language = self.opts.language.clone();
        s.lyrics = self.opts.lyrics;
        s.lyrics_online = self.opts.lyrics_online;
        s.auto_play = self.auto_play;
        s.burn_lyrics = self.burn_lyrics;
        s.out_dir = (self.opts.out_dir != default_dir).then(|| self.opts.out_dir.clone());
        self.store.save_settings(&self.settings);
    }

    fn running(&self) -> bool {
        self.rx.is_some()
    }

    /// Écrans de démonstration (option de compilation « demo » uniquement), pour vérifier le
    /// rendu sans lancer de traitement : MAGICMUS_DEMO=traitement | correction | resynchro.
    #[cfg(feature = "demo")]
    fn demo(&mut self, which: Option<&str>, ctx: &egui::Context) {
        match which {
            Some("traitement") => {
                let (tx, rx) = mpsc::channel();
                std::mem::forget(tx); // le « traitement » ne se termine jamais
                self.rx = Some(rx);
                self.opts.input = "https://www.youtube.com/watch?v=exemple".into();
                self.task = Task { stage: "Séparation voix / musique par IA".into(), progress: Some(0.42), step: 3 };
            }
            Some("correction") => {
                self.lyrics_editor = Some("un deux trois quatre\ncinq six sept huit\nneuf dix onze douze".into());
            }
            Some("bibliotheque") => self.lib_ui.tab = Tab::Library,
            Some("historique") => self.lib_ui.tab = Tab::History,
            // Avec un karaoké passé en argument : resynchronise ses paroles au démarrage.
            Some("resynchro") => {
                if let Some(p) = &self.player {
                    p.pause();
                }
                self.start_resync(ctx.clone(), None);
            }
            _ => {}
        }
    }

    fn highlight_color(&self) -> Color32 {
        th::HIGHLIGHTS[self.highlight].0
    }

    // ───────────────────────────── Traitement ─────────────────────────────

    fn start(&mut self, ctx: egui::Context) {
        self.log.clear();
        self.warnings.clear();
        self.error = None;
        self.task = Task { stage: "Démarrage".into(), progress: None, step: 0 };
        if let Some(p) = &self.player {
            p.pause();
        }

        let opts = self.opts.clone();
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        std::thread::spawn(move || {
            let mut emit = |e: Event| {
                let _ = tx.send(Msg::Event(e));
                ctx.request_repaint();
            };
            let res = pipeline::run(&opts, &mut emit).map_err(|e| format!("{e:#}"));
            let _ = tx.send(Msg::Finished(res));
            ctx.request_repaint();
        });
    }

    fn poll(&mut self) {
        let Some(rx) = &self.rx else { return };
        let mut msgs = Vec::new();
        let mut finished = None;
        loop {
            match rx.try_recv() {
                Ok(Msg::Event(e)) => msgs.push(e),
                Ok(Msg::Finished(r)) => {
                    finished = Some(r);
                    break;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    finished = Some(Err("le traitement s'est arrêté de façon inattendue".into()));
                    break;
                }
            }
        }
        for e in msgs {
            match e {
                Event::Stage(s) => {
                    self.log.push(format!("— {s}"));
                    if let Some(step) = step_of(&s) {
                        self.task.step = self.task.step.max(step);
                    }
                    self.task.stage = s;
                    self.task.progress = None;
                }
                Event::Progress(p) => self.task.progress = Some(p),
                Event::Log(l) => self.log.push(l),
                Event::Warning(w) => {
                    self.log.push(format!("Attention : {w}"));
                    self.warnings.push(w);
                }
            }
        }
        if self.log.len() > 1000 {
            self.log.drain(..self.log.len() - 1000);
        }
        if let Some(r) = finished {
            self.rx = None;
            match r {
                Ok(out) => {
                    // Les paroles saisies ont été enregistrées avec ce morceau.
                    self.opts.lyrics_text.clear();
                    self.paste_open = false;
                    let now = karaoke::library::now();
                    self.store.history.record_created(&out.output, &out.title, self.opts.input.trim(), now);
                    self.store.save_history();
                    self.load(out)
                }
                Err(e) => {
                    self.log.push(format!("Erreur : {e}"));
                    self.error = Some(e);
                }
            }
        }
    }

    /// Fichier ouvert par argument ou glisser-déposer : un karaoké déjà créé est lu
    /// directement, tout autre fichier devient la source à traiter.
    fn open_path(&mut self, path: PathBuf) {
        let name = path.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
        if name.starts_with("karaoke_") {
            self.load(Outcome::open(&path));
        } else {
            self.opts.input = path.display().to_string();
        }
    }

    /// Charge un karaoké dans le lecteur intégré.
    fn load(&mut self, out: Outcome) {
        self.player = None; // libère la sortie audio précédente
        self.export_result = None;
        match Player::open(&out.output, &out.title, out.has_video, out.lyrics, out.source.as_deref()) {
            Ok(p) => {
                self.store.history.record_played(&out.output, &out.title, karaoke::library::now());
                self.store.save_history();
                if self.auto_play {
                    p.play();
                }
                self.has_video = out.has_video;
                self.player = Some(p);
            }
            Err(e) => self.error = Some(format!("Lecture impossible : {e:#}")),
        }
    }

    // ──────────────────────────── Enregistrement ────────────────────────────

    fn start_export(&mut self, ctx: egui::Context) {
        let Some(p) = &self.player else { return };
        let (tempo, semitones) = (p.tempo(), p.semitones());
        let has_video = self.has_video;
        let ext = if has_video { "mp4" } else { "m4a" };
        let mut name = format!("{} - karaoké", p.title);
        if semitones != 0.0 {
            name.push_str(&format!(" {semitones:+} ton"));
        }
        if (tempo - 1.0).abs() > 1e-3 {
            name.push_str(&format!(" {}%", (tempo * 100.0).round()));
        }
        let name: String = name.chars().filter(|c| !r#"\/:*?"<>|"#.contains(*c)).collect();
        let mut dialog = rfd::FileDialog::new().set_file_name(format!("{name}.{ext}"));
        dialog = if has_video {
            dialog.add_filter("Vidéo MP4", &["mp4"]).add_filter("Audio seul", &["m4a", "mp3", "wav"])
        } else {
            dialog.add_filter("Audio", &["m4a", "mp3", "wav"])
        };
        if let Some(dir) = p.path.parent() {
            dialog = dialog.set_directory(dir);
        }
        let Some(dest) = dialog.save_file() else { return };

        let source = p.path.clone();
        let lyrics = p.lyrics.clone();
        let hl = self.highlight_color();
        let settings = ExportSettings { tempo, semitones, burn_lyrics: self.burn_lyrics, highlight: [hl.r(), hl.g(), hl.b()] };
        let (tx, rx) = mpsc::channel();
        self.export_rx = Some(rx);
        self.export_result = None;
        self.export_task = Task { stage: "Préparation".into(), ..Task::default() };
        std::thread::spawn(move || {
            let mut progress = |s: &str, p: Option<f32>| {
                let _ = tx.send(ExportMsg::Progress(s.to_string(), p));
                ctx.request_repaint();
            };
            let res = export::export(&source, has_video, lyrics.as_ref(), &settings, &dest, &mut progress)
                .map(|_| dest)
                .map_err(|e| format!("{e:#}"));
            let _ = tx.send(ExportMsg::Done(res));
            ctx.request_repaint();
        });
    }

    // ──────────────────────────── Resynchronisation ────────────────────────────

    /// Reconstruit les paroles du morceau ouvert (sans relancer Whisper) : paroles
    /// exactes en ligne, ou texte saisi / corrigé, recalées sur la voix entendue.
    /// `text` : texte corrigé à aligner (fenêtre « Corriger »), sinon texte conservé.
    fn start_resync(&mut self, ctx: egui::Context, text: Option<String>) {
        let Some(p) = &self.player else { return };
        let Some(dir) = p.path.parent().map(PathBuf::from) else { return };
        let old = p.lyrics.clone();
        // Un texte fourni ou corrigé par l'utilisateur est conservé.
        let user_text = text.or_else(|| {
            old.as_ref()
                .filter(|l| l.source == "texte saisi" || l.source == "texte corrigé")
                .map(|l| l.to_text())
        });
        let offset = old.as_ref().map_or(0.0, |l| l.offset);
        let language = old.as_ref().map(|l| l.language.clone()).filter(|l| !l.is_empty()).unwrap_or_else(|| "auto".into());
        let (title, duration, karaoke_path) = (p.title.clone(), p.duration(), p.path.clone());
        let (tx, rx) = mpsc::channel();
        self.resync_rx = Some(rx);
        self.resync_msg = None;
        self.resync_status = None;
        std::thread::spawn(move || {
            let mut warnings = Vec::new();
            let progress_tx = tx.clone();
            let progress_ctx = ctx.clone();
            let mut stage = String::new();
            let req = pipeline::LyricsRequest {
                work_dir: &dir,
                title: &title,
                artist: None,
                track: None,
                duration,
                language: &language,
                user_text: user_text.as_deref(),
                online: true,
                karaoke: Some(&karaoke_path),
            };
            let mut ly = pipeline::build_lyrics(&req, &mut |e| match e {
                Event::Warning(w) => warnings.push(w),
                Event::Stage(s) => {
                    stage = s;
                    let _ = progress_tx.send(ResyncMsg::Progress(stage.clone(), None));
                    progress_ctx.request_repaint();
                }
                Event::Progress(p) => {
                    let _ = progress_tx.send(ResyncMsg::Progress(stage.clone(), Some(p)));
                    progress_ctx.request_repaint();
                }
                Event::Log(_) => {}
            });
            if let (Some(ly), Some(_)) = (&mut ly, user_text.as_ref()) {
                ly.source = "texte corrigé".into();
            }
            if let Some(ly) = &mut ly {
                ly.offset = offset;
                let _ = ly.save(&dir.join(pipeline::LYRICS_FILE));
            }
            let _ = tx.send(ResyncMsg::Done(ly, warnings));
            ctx.request_repaint();
        });
    }

    fn poll_resync(&mut self) {
        let Some(rx) = &self.resync_rx else { return };
        let mut done = None;
        loop {
            match rx.try_recv() {
                Ok(ResyncMsg::Progress(s, p)) => self.resync_status = Some((s, p)),
                Ok(ResyncMsg::Done(ly, w)) => {
                    done = Some(Ok((ly, w)));
                    break;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    done = Some(Err(()));
                    break;
                }
            }
        }
        let Some(done) = done else { return };
        let msg = match done {
            Ok((Some(ly), _)) => {
                let n = ly.lines.len();
                if let Some(p) = &mut self.player {
                    p.lyrics = Some(ly);
                }
                self.show_lyrics = true;
                (format!("Paroles resynchronisées ({n} lignes)"), true)
            }
            Ok((None, warnings)) => (warnings.into_iter().next().unwrap_or_else(|| "Aucune parole trouvée".into()), false),
            Err(()) => ("La resynchronisation a échoué".into(), false),
        };
        self.resync_status = None;
        self.resync_rx = None;
        if !msg.1 {
            // Échec : bandeau d'avertissement en haut de l'écran.
            self.warnings.push(format!("Resynchronisation impossible : {}", msg.0));
        }
        self.resync_msg = Some((msg.0, msg.1, std::time::Instant::now()));
    }

    fn poll_export(&mut self) {
        let Some(rx) = &self.export_rx else { return };
        loop {
            match rx.try_recv() {
                Ok(ExportMsg::Progress(s, p)) => self.export_task = Task { stage: s, progress: p, step: 0 },
                Ok(ExportMsg::Done(r)) => {
                    self.export_result = Some(r);
                    self.export_rx = None;
                    break;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.export_result = Some(Err("l'enregistrement s'est arrêté".into()));
                    self.export_rx = None;
                    break;
                }
            }
        }
    }

    // ───────────────────────────── Panneau gauche ─────────────────────────────

    fn side_panel(&mut self, ui: &mut egui::Ui) {
        // Marque
        egui::Panel::top("marque")
            .show_separator_line(false)
            .frame(Frame::new().inner_margin(Margin { left: 24, right: 24, top: 24, bottom: 18 }))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    th::logo_disc(ui, &self.logo, 58.0, 52.0, th::ROSE_SOFT, 3.0);
                    ui.add_space(4.0);
                    ui.vertical(|ui| {
                        ui.add_space(6.0);
                        let mut job = LayoutJob::default();
                        job.append("Magic", 0.0, fmt(30.0, W::Serif, th::INK));
                        job.append("Mus", 0.0, fmt(30.0, W::SerifItalic, th::ROSE));
                        ui.label(job);
                        ui.label(th::t("Le karaoké qui fait disparaître la voix", 13.0, W::Medium, th::MUTED));
                    });
                });
                ui.add_space(14.0);
                ui_library::tabs(ui, &mut self.lib_ui.tab);
            });

        let tab = self.lib_ui.tab;
        if tab != Tab::Create {
            egui::CentralPanel::default()
                .frame(Frame::new().inner_margin(Margin { left: 18, right: 14, top: 0, bottom: 8 }))
                .show(ui, |ui| {
                    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                        ui.set_width(ui.available_width() - 6.0);
                        ui.spacing_mut().item_spacing.y = 4.0;
                        if tab == Tab::Library {
                            self.library_panel(ui);
                        } else {
                            self.history_panel(ui);
                        }
                    });
                });
            return;
        }

        // Bouton de création, toujours visible en bas
        egui::Panel::bottom("creer")
            .show_separator_line(false)
            .frame(Frame::new().fill(th::PANEL).inner_margin(Margin { left: 24, right: 24, top: 16, bottom: 18 }))
            .show(ui, |ui| {
                let top = ui.max_rect().top() - 16.0;
                ui.painter().hline(ui.max_rect().x_range().expand(24.0), top, Stroke::new(1.0, th::BORDER));
                let running = self.running();
                let can_start = !running && !self.opts.input.trim().is_empty();
                let label = if running { "Création en cours…" } else { "Créer le karaoké" };
                if th::primary(ui, label, Vec2::new(ui.available_width(), 50.0), 16.0, can_start).clicked() {
                    self.start(ui.ctx().clone());
                }
                ui.add_space(4.0);
                ui.vertical_centered(|ui| {
                    if th::link(ui, "Ouvrir un karaoké déjà créé…", th::MUTED).clicked() {
                        if let Some(p) = rfd::FileDialog::new()
                            .add_filter("Karaoké", &["mp4", "m4a"])
                            .set_directory(&self.opts.out_dir)
                            .pick_file()
                        {
                            self.load(Outcome::open(&p));
                        }
                    }
                });
            });

        // Formulaire défilant
        egui::CentralPanel::default()
            .frame(Frame::new().inner_margin(Margin { left: 24, right: 18, top: 4, bottom: 8 }))
            .show(ui, |ui| {
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    ui.set_width(ui.available_width() - 6.0);
                    let running = self.running();
                    if running {
                        ui.multiply_opacity(0.5);
                    }
                    ui.add_enabled_ui(!running, |ui| self.form(ui));
                });
            });
    }

    fn form(&mut self, ui: &mut egui::Ui) {
        ui.spacing_mut().item_spacing.y = 10.0;

        // Source
        th::section_label(ui, "Vidéo YouTube ou fichier", th::FAINT);
        ui.add(
            egui::TextEdit::singleline(&mut self.opts.input)
                .hint_text("https://www.youtube.com/watch?v=…")
                .font(th::font(14.0, W::Medium))
                .text_color(th::INK)
                .margin(Margin::symmetric(14, 12))
                .desired_width(f32::INFINITY),
        );
        ui.horizontal(|ui| {
            if th::secondary(ui, "Choisir un fichier…").clicked() {
                if let Some(p) = rfd::FileDialog::new()
                    .add_filter("Audio / vidéo", &["mp4", "mkv", "webm", "mov", "avi", "mp3", "m4a", "wav", "flac", "ogg", "opus"])
                    .pick_file()
                {
                    self.opts.input = p.display().to_string();
                }
            }
            ui.label(th::t("ou glissez-le sur la fenêtre", 12.0, W::Medium, th::FAINT));
        });

        // Méthode
        ui.add_space(12.0);
        th::section_label(ui, "Suppression de la voix", th::FAINT);
        ui.horizontal(|ui| {
            let w = (ui.available_width() - 10.0) / 2.0;
            if th::choice_card(ui, self.opts.method == Method::Ia, "Qualité", "Par IA · ≈ 1 min par chanson", w).clicked() {
                self.opts.method = Method::Ia;
            }
            if th::choice_card(ui, self.opts.method == Method::Spectral, "Rapide", "Canal central · quelques s", w).clicked() {
                self.opts.method = Method::Spectral;
            }
        });
        if self.opts.method == Method::Spectral {
            ui.horizontal(|ui| {
                ui.label(th::t("Intensité", 13.0, W::Bold, th::INK));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(th::t(format!("{:.2}", self.opts.strength).replace('.', ","), 13.0, W::Bold, th::ROSE));
                });
            });
            let mut v = self.opts.strength as f64;
            if th::slider(ui, &mut v, 0.0..=1.0, 0.01, 0.0, ui.available_width()).changed() {
                self.opts.strength = v as f32;
            }
        }

        // Paroles
        ui.add_space(12.0);
        th::section_label(ui, "Paroles", th::FAINT);
        th::switch_row(ui, "Paroles synchronisées", &mut self.opts.lyrics);
        let lyrics_on = self.opts.lyrics;
        ui.scope(|ui| {
            if !lyrics_on {
                ui.multiply_opacity(0.4);
            }
            ui.add_enabled_ui(lyrics_on, |ui| {
                ui.horizontal(|ui| {
                    ui.label(th::t("Langue", 14.0, W::Semi, th::INK));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let current = LANGUAGES.iter().find(|(c, _)| *c == self.opts.language).map_or("?", |(_, n)| *n);
                        egui::ComboBox::from_id_salt("langue")
                            .selected_text(th::t(current, 13.0, W::Semi, th::INK))
                            .width(180.0)
                            .show_ui(ui, |ui| {
                                for (code, name) in LANGUAGES {
                                    ui.selectable_value(&mut self.opts.language, code.to_string(), name);
                                }
                            });
                    });
                });
                th::switch_row(ui, "Chercher les paroles exactes en ligne (LRCLIB)", &mut self.opts.lyrics_online);
                if disclosure(ui, "Coller les paroles exactes (facultatif)", self.paste_open, 13.0, W::Bold, th::ROSE).clicked() {
                    self.paste_open = !self.paste_open;
                }
                if self.paste_open {
                    ui.add(
                        egui::TextEdit::multiline(&mut self.opts.lyrics_text)
                            .hint_text("Une ligne par vers. Les horodatages LRC [mm:ss.xx] sont acceptés.")
                            .font(th::font(13.0, W::Medium))
                            .margin(Margin::symmetric(12, 10))
                            .desired_rows(5)
                            .desired_width(f32::INFINITY),
                    );
                }
                ui.add(
                    egui::Label::new(th::t(
                        "Le minutage de chaque mot vient de la reconnaissance vocale (Whisper, ≈ 550 Mo au premier usage).",
                        12.0,
                        W::Medium,
                        th::FAINT,
                    ))
                    .wrap(),
                );
            });
        });

        // Options
        ui.add_space(12.0);
        if disclosure(ui, "OPTIONS", self.options_open, 11.0, W::Black, th::FAINT).clicked() {
            self.options_open = !self.options_open;
        }
        if self.options_open {
            th::checkbox(ui, &mut self.opts.audio_only, "Audio seulement (pas de vidéo)", true);
            th::checkbox(ui, &mut self.auto_play, "Lancer la lecture à la fin", true);
            th::checkbox(ui, &mut self.opts.force, "Tout refaire, même si déjà traité", true);
            Frame::new().fill(th::BG).corner_radius(CornerRadius::same(10)).inner_margin(Margin::symmetric(10, 8)).show(ui, |ui| {
                ui.horizontal(|ui| {
                    let change = ui
                        .with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let r = th::link(ui, "Changer…", th::ROSE);
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(self.opts.out_dir.display().to_string()).monospace().size(12.0).color(th::MUTED),
                                )
                                .truncate(),
                            );
                            r
                        })
                        .inner;
                    if change.clicked() {
                        if let Some(d) = rfd::FileDialog::new().set_directory(&self.opts.out_dir).pick_folder() {
                            self.opts.out_dir = d;
                        }
                    }
                });
            });

            ui.add_space(4.0);
            ui.label(th::t("Couleur des mots chantés", 14.0, W::Semi, th::INK));
            ui.horizontal(|ui| {
                for (i, (c, name)) in th::HIGHLIGHTS.iter().enumerate() {
                    let (r, resp) = ui.allocate_exact_size(Vec2::new(44.0, 30.0), Sense::click());
                    let sel = self.highlight == i;
                    ui.painter().rect(
                        r,
                        CornerRadius::same(8),
                        *c,
                        Stroke::new(if sel { 2.0 } else { 1.0 }, if sel { th::INK } else { th::BORDER_INPUT }),
                        egui::StrokeKind::Inside,
                    );
                    if resp.on_hover_text(*name).clicked() {
                        self.highlight = i;
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.label(th::t("Taille des paroles", 14.0, W::Semi, th::INK));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(th::t(format!("{} px", self.lyrics_size as i32), 13.0, W::Bold, th::ROSE));
                });
            });
            let mut size = self.lyrics_size as f64;
            if th::slider(ui, &mut size, 24.0..=52.0, 2.0, 24.0, ui.available_width()).changed() {
                self.lyrics_size = size as f32;
            }

            if disclosure(ui, "Journal", self.journal_open, 13.0, W::Bold, th::MUTED).clicked() {
                self.journal_open = !self.journal_open;
            }
            if self.journal_open {
                egui::ScrollArea::vertical().id_salt("journal").max_height(200.0).stick_to_bottom(true).show(ui, |ui| {
                    for l in &self.log {
                        ui.label(egui::RichText::new(l).monospace().size(11.0).color(th::MUTED));
                    }
                });
            }
        }
    }

    // ───────────────────────────── Zone principale ─────────────────────────────

    /// Avertissements et erreurs, au-dessus de l'écran courant.
    fn banners(&mut self, ui: &mut egui::Ui) {
        let mut close = false;
        if let Some(e) = &self.error {
            banner(ui, &format!("Erreur : {e}"), Color32::from_rgb(0xfb, 0xe9, 0xec), Color32::from_rgb(0xa3, 0x2b, 0x48), &mut close);
            if close {
                self.error = None;
            }
        }
        if !self.running() {
            let mut i = 0;
            while i < self.warnings.len() {
                let mut c = false;
                banner(ui, &self.warnings[i], Color32::from_rgb(0xfd, 0xf3, 0xe1), Color32::from_rgb(0x8a, 0x5a, 0x00), &mut c);
                if c {
                    self.warnings.remove(i);
                } else {
                    i += 1;
                }
            }
        }
    }

    fn welcome(&mut self, ui: &mut egui::Ui) {
        let avail = ui.available_height();
        ui.vertical_centered(|ui| {
            ui.add_space(((avail - 560.0) / 2.0).max(0.0));
            let (rect, _) = ui.allocate_exact_size(Vec2::splat(260.0), Sense::hover());
            th::radial_disc(ui.painter(), rect.center(), 130.0, th::ROSE_SOFT, th::BG);
            th::paint_logo(ui.painter(), &self.logo, rect.center(), 230.0);
            ui.add_space(22.0);
            let mut job = LayoutJob::default();
            job.append("Une chanson, un coup de chapeau, ", 0.0, fmt(44.0, W::Serif, th::INK));
            job.append("et la voix s'envole.", 0.0, fmt(44.0, W::SerifItalic, th::ROSE));
            job.wrap.max_width = 560.0;
            job.halign = Align::Center;
            // Mise en page préalable : la largeur de 560 px est respectée.
            let galley = ui.painter().layout_job(job);
            ui.label(galley);
            ui.add_space(14.0);
            ui.add_sized(
                [460.0, 50.0],
                egui::Label::new(th::t(
                    "Collez une adresse YouTube à gauche puis cliquez sur « Créer le karaoké ». La vidéo s'affichera ici.",
                    16.0,
                    W::Medium,
                    th::MUTED,
                ))
                .wrap()
                .halign(Align::Center),
            );
            ui.add_space(14.0);
            centered_row(ui, 300.0, |ui| {
                th::chip(ui, "Aucune installation", None);
                th::chip(ui, "Glisser-déposer accepté", None);
            });
        });
    }

    fn processing(&mut self, ui: &mut egui::Ui) {
        let steps: Vec<usize> = (0..STEPS.len()).filter(|i| self.opts.lyrics || !LYRICS_STEPS.contains(i)).collect();
        let cur = steps.iter().position(|&s| s >= self.task.step).unwrap_or(steps.len() - 1);
        let step_p = self.task.progress.unwrap_or(0.0);
        let total = (cur as f32 + step_p) / steps.len() as f32;
        let time = ui.input(|i| i.time);
        let avail = ui.available_height();

        ui.vertical_centered(|ui| {
            ui.add_space(((avail - 600.0) / 2.0).max(0.0));
            let bob = (time * 1.6).sin() as f32 * 5.0;
            th::logo_disc(ui, &self.logo, 170.0, 150.0, th::ROSE_SOFT, bob);
            ui.add_space(18.0);
            let mut job = LayoutJob::default();
            job.append("Un peu de ", 0.0, fmt(38.0, W::Serif, th::INK));
            job.append("magie", 0.0, fmt(38.0, W::SerifItalic, th::ROSE));
            job.append("…", 0.0, fmt(38.0, W::Serif, th::INK));
            ui.label(job);
            ui.add_space(18.0);

            centered_row(ui, 440.0, |ui| {
                ui.vertical(|ui| {
                    ui.set_width(440.0);
                    ui.horizontal(|ui| {
                        ui.add(egui::Label::new(th::t(&self.task.stage, 14.0, W::Bold, th::INK)).truncate());
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let pct = self.task.progress.map_or(String::new(), |p| format!("{} %", (p * 100.0).round()));
                            ui.label(th::t(pct, 14.0, W::Bold, th::ROSE));
                        });
                    });
                    th::progress(ui, total, 440.0, 10.0);
                    ui.add_space(8.0);
                    th::card(16, Margin::symmetric(18, 14)).show(ui, |ui| {
                        ui.set_width(440.0 - 36.0);
                        ui.spacing_mut().item_spacing.y = 9.0;
                        for (k, &s) in steps.iter().enumerate() {
                            let (done, now) = (k < cur, k == cur);
                            ui.horizontal(|ui| {
                                let (r, _) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::hover());
                                let c = r.center();
                                let p = ui.painter();
                                if done {
                                    p.circle_filled(c, 10.0, th::ROSE);
                                    p.line(
                                        vec![c + Vec2::new(-4.5, 0.2), c + Vec2::new(-1.2, 3.4), c + Vec2::new(4.6, -3.4)],
                                        Stroke::new(2.0, Color32::WHITE),
                                    );
                                } else {
                                    p.circle(c, 9.25, if now { th::ROSE_SOFT } else { Color32::TRANSPARENT }, Stroke::new(1.5, if now { th::ROSE } else { th::STEP_BORDER }));
                                }
                                let color = if done { th::MUTED } else if now { th::INK } else { th::STEP_TODO };
                                ui.label(th::t(STEPS[s], 14.0, W::Semi, color));
                            });
                        }
                    });
                    for w in &self.warnings {
                        ui.add(egui::Label::new(th::t(format!("⚠ {w}"), 12.0, W::Semi, Color32::from_rgb(0x8a, 0x5a, 0x00))).wrap());
                    }
                });
            });
        });
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
    }

    fn player_ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let Some(p) = &mut self.player else { return };
        let duration = p.duration();
        let t = self.seek_drag.unwrap_or_else(|| p.position());
        p.update_video(&ctx, t);
        ui.spacing_mut().item_spacing.y = 16.0;

        // En-tête : titre, origine, dossier
        let method = if p.path.file_name().is_some_and(|n| n.to_string_lossy().contains("rapide")) { "rapide" } else { "IA" };
        let mut origin = format!("Karaoké {method}");
        if let Some(src) = p.lyrics.as_ref().map(|l| l.source.clone()).filter(|s| !s.is_empty()) {
            origin.push_str(&format!(" · paroles {src}"));
        }
        let mut reveal = false;
        ui.horizontal(|ui| {
            let right = 580.0;
            ui.vertical(|ui| {
                ui.set_width((ui.available_width() - right).max(200.0));
                ui.spacing_mut().item_spacing.y = 6.0;
                th::section_label(ui, "En scène", th::ROSE);
                ui.add(egui::Label::new(th::t(&p.title, 30.0, W::Serif, th::INK)).truncate());
            });
            ui.with_layout(Layout::right_to_left(Align::Max), |ui| {
                reveal = th::secondary(ui, "Dossier").on_hover_text("Afficher dans l'Explorateur").clicked();
                ui_library::library_button(ui, &mut self.store, &mut self.lib_ui, &p.path, &p.title);
                th::chip(ui, &origin, Some(th::PANEL));
            });
        });
        if reveal {
            let _ = media::reveal(&p.path);
        }

        // Image, paroles et repères
        let below = 72.0 + 76.0 + 36.0 + 3.0 * 16.0;
        let w = ui.available_width();
        let h = (ui.available_height() - below).max(160.0);
        let (area, resp) = ui.allocate_exact_size(Vec2::new(w, h), Sense::click());
        let painter = ui.painter_at(area);
        painter.rect_filled(area, CornerRadius::same(22), th::VIDEO_BG);
        if let Some(tex) = p.texture() {
            let a = p.aspect();
            let (iw, ih) = if area.width() / area.height() > a { (area.height() * a, area.height()) } else { (area.width(), area.width() / a) };
            let img = Rect::from_center_size(area.center(), Vec2::new(iw, ih));
            let fills = (iw - area.width()).abs() < 2.0 && (ih - area.height()).abs() < 2.0;
            egui::Image::from_texture((tex.id(), img.size()))
                .corner_radius(if fills { 22 } else { 0 })
                .paint_at(ui, img);
        } else if !p.has_video() {
            th::paint_logo(&painter, &self.logo, area.center() - Vec2::new(0.0, h * 0.12), h * 0.38);
        }
        let lyrics_visible = self.show_lyrics && p.lyrics.is_some();
        if lyrics_visible {
            // Dégradé sombre sous les paroles (les coins arrondis restent intacts).
            let g_top = area.bottom() - (h * 0.45).min(260.0);
            let shade = Color32::from_rgba_unmultiplied(29, 24, 32, 217);
            th::vertical_gradient(&painter, Rect::from_min_max(Pos2::new(area.left(), g_top), Pos2::new(area.right(), area.bottom() - 22.0)), Color32::TRANSPARENT, shade);
            painter.rect_filled(
                Rect::from_min_max(Pos2::new(area.left(), area.bottom() - 22.0), area.max),
                CornerRadius { nw: 0, ne: 0, sw: 22, se: 22 },
                shade,
            );
            let style = LyricsStyle {
                size: self.lyrics_size * (h / 470.0).clamp(0.6, 1.6),
                highlight: th::HIGHLIGHTS[self.highlight].0,
                current: th::family(W::Black),
                next: th::family(W::Bold),
            };
            if let Some(ly) = &p.lyrics {
                player::draw_lyrics(&painter, area.shrink2(Vec2::new(40.0, 0.0)), ly, t, &style);
            }
        }
        if p.original() {
            pill(&painter, area.left_top() + Vec2::new(16.0, 16.0), Align2::LEFT_TOP, "VERSION ORIGINALE", th::ROSE, Color32::WHITE, true);
        }
        if !p.is_playing() {
            pill(&painter, area.right_top() + Vec2::new(-16.0, 16.0), Align2::RIGHT_TOP, "En pause · Espace pour reprendre", Color32::from_white_alpha(30), Color32::WHITE, false);
        }
        if resp.clicked() {
            p.toggle();
        }

        // Barre de lecture
        th::card(18, Margin::symmetric(16, 12)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 16.0;
                if th::play_button(ui, p.is_playing()).clicked() {
                    p.toggle();
                }
                let mut job = LayoutJob::default();
                job.append(&fmt_time(t), 0.0, fmt(14.0, W::Bold, th::INK));
                job.append(&format!(" / {}", fmt_time(duration)), 0.0, fmt(14.0, W::Semi, th::FAINT));
                ui.label(job);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    th::keycap(ui, "V");
                    let mut original = p.original();
                    let ready = p.original_available();
                    let r = th::segmented(ui, &mut original, "Voix retirée", "Originale", ready)
                        .on_hover_text("Basculer entre le karaoké et la version originale (touche V)")
                        .on_disabled_hover_text("Version originale en cours de chargement…");
                    if r.changed() {
                        p.set_original(original);
                    }
                    ui.add_space(8.0);
                    let mut pos = t;
                    let r = th::slider(ui, &mut pos, 0.0..=duration.max(0.1), 0.0, 0.0, ui.available_width());
                    if r.dragged() {
                        self.seek_drag = Some(pos);
                    }
                    if r.drag_stopped() || (r.changed() && !r.dragged()) {
                        p.seek(pos);
                        self.seek_drag = None;
                    }
                });
            });
        });

        // Tonalité, vitesse, volume
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 12.0;
            let reset_w = 120.0;
            let cw = (ui.available_width() - reset_w - 3.0 * 12.0) / 3.0;
            let semi = p.semitones() as f64;
            let semi_label = if semi == 0.0 {
                "0".to_string()
            } else {
                format!("{}{} ½ ton", if semi > 0.0 { "+" } else { "−" }, format!("{}", semi.abs()).replace('.', ","))
            };
            if let Some(v) = knob(ui, cw, "Tonalité", &semi_label, semi, -12.0..=12.0, 0.5, 0.0) {
                p.set_semitones(v as f32);
            }
            let tempo = (p.tempo() * 100.0).round() as f64;
            if let Some(v) = knob(ui, cw, "Vitesse", &format!("{tempo} %"), tempo, 50.0..=150.0, 5.0, 100.0) {
                p.set_tempo(v as f32 / 100.0);
            }
            let vol = (p.volume() * 100.0).round() as f64;
            if let Some(v) = knob(ui, cw, "Volume", &format!("{vol} %"), vol, 0.0..=150.0, 1.0, 0.0) {
                p.set_volume(v as f32 / 100.0);
            }
            if th::secondary_sized(ui, "Réinitialiser", Vec2::new(reset_w, 76.0), 16)
                .on_hover_text("Tonalité et vitesse d'origine")
                .clicked()
            {
                p.set_semitones(0.0);
                p.set_tempo(1.0);
            }
        });

        // Paroles et enregistrement
        let has_lyrics = p.lyrics.is_some();
        let (mut open_editor, mut do_export, mut do_resync) = (false, false, false);
        let exporting = self.export_rx.is_some();
        let resyncing = self.resync_rx.is_some();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 14.0;
            ui.scope(|ui| {
                if !has_lyrics {
                    ui.multiply_opacity(0.45);
                }
                ui.spacing_mut().item_spacing.x = 10.0;
                th::switch(ui, &mut self.show_lyrics).on_hover_text("Afficher les paroles sur l'image");
                ui.label(th::t("Paroles", 13.0, W::Bold, th::INK));
            });
            if let Some(ly) = &mut p.lyrics {
                ui.label(th::t("Décalage", 13.0, W::Bold, th::MUTED))
                    .on_hover_text("Positif : les paroles s'allument plus tôt ; négatif : plus tard.");
                let label = format!(
                    "{}{} s",
                    if ly.offset > 0.0 { "+" } else if ly.offset < 0.0 { "−" } else { "" },
                    format!("{:.2}", ly.offset.abs()).replace('.', ",")
                );
                if th::stepper(ui, &mut ly.offset, 0.05, -5.0..=5.0, &label) {
                    if let Some(dir) = p.path.parent() {
                        let _ = ly.save(&dir.join(pipeline::LYRICS_FILE));
                    }
                }
            }
            open_editor = th::secondary(ui, "Corriger…").on_hover_text("Corriger le texte des paroles").clicked();
            if resyncing {
                ui.spinner();
                // Étape en cours (téléchargement du modèle au premier usage, calcul…).
                let label = match &self.resync_status {
                    Some((s, Some(p))) if s.starts_with("Téléchargement") => format!("Modèle d'alignement… {:.0} %", p * 100.0),
                    Some((_, Some(p))) => format!("Alignement… {:.0} %", p * 100.0),
                    _ => "Synchronisation…".to_string(),
                };
                ui.label(th::t(label, 13.0, W::Bold, th::MUTED))
                    .on_hover_text(self.resync_status.as_ref().map_or(String::new(), |s| s.0.clone()));
            } else {
                // Après une réussite, le bouton l'indique quelques secondes.
                let done = self.resync_msg.as_ref().filter(|m| m.1 && m.2.elapsed().as_secs() < 6);
                let label = if done.is_some() { "Resynchronisé" } else { "Resynchroniser" };
                let hover = match done {
                    Some(m) => m.0.clone(),
                    None => "Recale les paroles sur la voix entendue (paroles exactes en ligne ou texte corrigé + \
                             reconnaissance vocale). Remet le décalage à 0."
                        .into(),
                };
                do_resync = th::secondary(ui, label).on_hover_text(hover).clicked();
                if done.is_some() {
                    ctx.request_repaint_after(std::time::Duration::from_secs(1));
                }
            }

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 14.0;
                do_export = !exporting
                    && th::primary_fit(ui, "Enregistrer…", 14.0, 38.0)
                        .on_hover_text("Enregistre le karaoké avec la tonalité et la vitesse actuelles")
                        .clicked();
                if self.has_video {
                    th::checkbox(ui, &mut self.burn_lyrics, "Incruster", has_lyrics)
                        .on_hover_text("Incruster les paroles dans la vidéo enregistrée");
                }
                if exporting {
                    let frac = self.export_task.progress.unwrap_or(0.0);
                    th::progress(ui, frac, 110.0, 6.0);
                    ui.label(th::t("Encodage", 13.0, W::Bold, th::MUTED));
                } else {
                    match &self.export_result {
                        Some(Ok(path)) => {
                            if th::link(ui, "Afficher", th::ROSE).clicked() {
                                let _ = media::reveal(path);
                            }
                            ui.label(th::t("Enregistré ·", 13.0, W::Bold, th::ROSE));
                        }
                        Some(Err(e)) => {
                            ui.add(egui::Label::new(th::t(format!("Échec : {e}"), 12.0, W::Bold, Color32::from_rgb(0xa3, 0x2b, 0x48))).truncate());
                        }
                        None => {}
                    }
                }
            });
        });

        if open_editor {
            self.lyrics_editor = Some(p.lyrics.as_ref().map(|l| l.to_text()).unwrap_or_default());
        }
        if do_export {
            self.start_export(ctx.clone());
        }
        if do_resync {
            self.start_resync(ctx.clone(), None);
        }
        if self.player.as_ref().is_some_and(|p| p.is_playing()) {
            ctx.request_repaint();
        }
    }

    /// Raccourcis du lecteur (si aucun champ de texte n'a le focus).
    fn shortcuts(&mut self, ctx: &egui::Context) {
        if self.lyrics_editor.is_some() || ctx.memory(|m| m.focused().is_some()) {
            return;
        }
        let Some(p) = &mut self.player else { return };
        let (space, left, right, voice) = ctx.input(|i| {
            (
                i.key_pressed(egui::Key::Space),
                i.key_pressed(egui::Key::ArrowLeft),
                i.key_pressed(egui::Key::ArrowRight),
                i.key_pressed(egui::Key::V),
            )
        });
        if space {
            p.toggle();
        }
        if voice {
            p.set_original(!p.original());
        }
        if left {
            p.seek(p.position() - 5.0);
        }
        if right {
            p.seek(p.position() + 5.0);
        }
    }

    /// Fenêtre de correction des paroles : le texte corrigé est recalé sur la
    /// reconnaissance vocale (et sur le début des vers existants si leur nombre est inchangé).
    fn lyrics_editor_window(&mut self, ctx: &egui::Context) {
        let Some(text) = &mut self.lyrics_editor else { return };
        let (mut apply, mut cancel) = (false, false);
        let logo = self.logo.clone();
        let modal = egui::Modal::new(egui::Id::unique("corriger-paroles"))
            .backdrop_color(Color32::from_rgba_unmultiplied(36, 28, 37, 97))
            .frame(Frame::new().fill(th::PANEL).corner_radius(CornerRadius::same(22)).inner_margin(Margin::same(28)))
            .show(ctx, |ui| {
                ui.set_width(524.0);
                ui.spacing_mut().item_spacing.y = 14.0;
                ui.horizontal(|ui| {
                    th::logo_disc(ui, &logo, 44.0, 40.0, th::ROSE_SOFT, 2.0);
                    ui.label(th::t("Corriger les paroles", 28.0, W::Serif, th::INK));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let (r, resp) = ui.allocate_exact_size(Vec2::splat(32.0), Sense::click());
                        ui.painter().circle_filled(r.center(), 16.0, th::SEGMENT_BG);
                        ui.painter().text(r.center(), Align2::CENTER_CENTER, "×", th::font(18.0, W::Bold), if resp.hovered() { th::ROSE } else { th::MUTED });
                        cancel |= resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked();
                    });
                });
                ui.add(
                    egui::Label::new(th::t(
                        "Une ligne par vers. Corrigez les mots, ajoutez ou supprimez des lignes, ou collez les paroles exactes. Le texte sera recalé automatiquement.",
                        14.0,
                        W::Medium,
                        th::MUTED,
                    ))
                    .wrap(),
                );
                egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                    ui.add(
                        egui::TextEdit::multiline(text)
                            .font(th::font(15.0, W::Semi))
                            .margin(Margin::symmetric(16, 14))
                            .desired_rows(14)
                            .desired_width(f32::INFINITY),
                    );
                });
                ui.label(th::t("Les horodatages LRC « [mm:ss.xx] » en début de ligne sont acceptés.", 12.0, W::Medium, th::FAINT));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    apply = th::primary_fit(ui, "Appliquer", 14.0, 40.0).clicked();
                    cancel |= th::secondary_sized(ui, "Annuler", Vec2::new(100.0, 40.0), 12).clicked();
                });
            });
        if modal.should_close() {
            cancel = true;
        }

        if apply {
            // Le texte corrigé est aligné en arrière-plan (alignement forcé sur la voix),
            // comme une resynchronisation.
            if let Some(text) = self.lyrics_editor.clone() {
                self.start_resync(ctx.clone(), Some(text));
            }
        }
        if apply || cancel {
            self.lyrics_editor = None;
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        trace("image");
        self.poll();
        self.poll_export();
        self.poll_resync();
        let ctx = ui.ctx().clone();
        let dropped: Vec<PathBuf> =
            ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| p.is_file()).collect());
        if let Some(path) = dropped.into_iter().next() {
            if !self.running() {
                self.open_path(path);
            }
        }

        egui::Panel::left("formulaire")
            .show_separator_line(false)
            .resizable(false)
            .default_size(360.0)
            .min_size(360.0)
            .frame(Frame::new().fill(th::PANEL).stroke(Stroke::new(1.0, th::BORDER)))
            .show(ui, |ui| self.side_panel(ui));
        egui::CentralPanel::default()
            .frame(Frame::new().fill(th::BG).inner_margin(Margin::same(28)))
            .show(ui, |ui| {
                self.migration_banner(ui);
                self.banners(ui);
                if self.running() {
                    self.processing(ui);
                } else if self.player.is_some() {
                    self.player_ui(ui);
                } else {
                    self.welcome(ui);
                }
            });

        self.lyrics_editor_window(&ctx);
        self.shortcuts(&ctx);
        self.sync_settings();
    }
}

// ───────────────────────────── Petits éléments ─────────────────────────────

fn fmt(size: f32, w: W, color: Color32) -> TextFormat {
    TextFormat { font_id: th::font(size, w), color, ..Default::default() }
}

fn fmt_time(t: f64) -> String {
    let s = t.max(0.0) as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

/// Libellé dépliable précédé d'un chevron « › ».
fn disclosure(ui: &mut egui::Ui, label: &str, open: bool, size: f32, w: W, color: Color32) -> egui::Response {
    let text = if open { format!("⌄  {label}") } else { format!("›  {label}") };
    let mut rt = th::t(text, size, w, color);
    if size <= 11.0 {
        rt = rt.extra_letter_spacing(1.3);
    }
    ui.add(egui::Label::new(rt).sense(Sense::click())).on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Rangée centrée horizontalement (largeur approximative du contenu).
fn centered_row(ui: &mut egui::Ui, width: f32, add: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.add_space(((ui.available_width() - width) / 2.0).max(0.0));
        add(ui);
    });
}

/// Carte de réglage du lecteur (libellé, valeur, curseur). Renvoie la nouvelle valeur.
#[allow(clippy::too_many_arguments)]
fn knob(
    ui: &mut egui::Ui,
    width: f32,
    label: &str,
    value_label: &str,
    value: f64,
    range: std::ops::RangeInclusive<f64>,
    step: f64,
    origin: f64,
) -> Option<f64> {
    let mut out = None;
    th::card(16, Margin::symmetric(16, 12)).show(ui, |ui| {
        let inner = width - 32.0;
        ui.set_width(inner);
        ui.spacing_mut().item_spacing.y = 10.0;
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                ui.label(th::t(label, 13.0, W::Bold, th::MUTED));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(th::t(value_label, 15.0, W::Black, th::INK));
                });
            });
            let mut v = value;
            if th::slider(ui, &mut v, range, step, origin, inner).changed() {
                out = Some(v);
            }
        });
    });
    out
}

/// Pastille dessinée sur l'image.
fn pill(painter: &egui::Painter, anchor: Pos2, align: Align2, text: &str, bg: Color32, fg: Color32, caps: bool) {
    let f = th::font(12.0, if caps { W::Black } else { W::Bold });
    let galley = painter.layout_no_wrap(text.to_string(), f, fg);
    let size = galley.size() + Vec2::new(28.0, 18.0);
    let rect = align.anchor_size(anchor, size);
    painter.rect_filled(rect, CornerRadius::same(255), bg);
    painter.galley(rect.center() - galley.size() / 2.0, galley, fg);
}

/// Bandeau d'information refermable.
fn banner(ui: &mut egui::Ui, text: &str, bg: Color32, fg: Color32, close: &mut bool) {
    Frame::new().fill(bg).corner_radius(CornerRadius::same(12)).inner_margin(Margin::symmetric(14, 10)).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.set_width(ui.available_width());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if th::link(ui, "×", fg).clicked() {
                    *close = true;
                }
                ui.add(egui::Label::new(th::t(text, 13.0, W::Semi, fg)).wrap());
            });
        });
    });
    ui.add_space(8.0);
}
