//! Application graphique : télécharge une vidéo YouTube, transcrit les paroles,
//! supprime la voix et lit le karaoké dans la fenêtre (tonalité et vitesse réglables).

// Pas de fenêtre de console en version release.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui::{self, Color32, Pos2, Rect, RichText, Sense};
use karaoke::export::{self, ExportSettings};
use karaoke::pipeline::{self, Event, Method, Options, Outcome};
use karaoke::player::{self, Player};
use karaoke::{media, tools};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};

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

fn main() -> eframe::Result {
    tools::setup_path();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Karaoké")
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([900.0, 600.0]),
        ..Default::default()
    };
    // Un fichier passé en argument (« Ouvrir avec… ») est ouvert directement.
    let arg = std::env::args_os().nth(1).map(PathBuf::from);
    eframe::run_native(
        "Karaoké",
        options,
        Box::new(move |_cc| {
            let mut app = App::default();
            if let Some(path) = arg.filter(|p| p.is_file()) {
                app.open_path(path);
            }
            Ok(Box::new(app))
        }),
    )
}

enum Msg {
    Event(Event),
    Finished(Result<Outcome, String>),
}

/// Tâche de fond (traitement ou enregistrement) et son état d'avancement.
#[derive(Default)]
struct Task {
    stage: String,
    progress: Option<f32>,
}

struct App {
    opts: Options,
    auto_play: bool,
    rx: Option<Receiver<Msg>>,
    task: Task,
    log: Vec<String>,
    warnings: Vec<String>,
    error: Option<String>,

    player: Option<Player>,
    has_video: bool,
    show_lyrics: bool,
    seek_drag: Option<f64>,

    burn_lyrics: bool,
    lyrics_editor: Option<String>,
    export_rx: Option<Receiver<ExportMsg>>,
    export_task: Task,
    export_result: Option<Result<PathBuf, String>>,
}

enum ExportMsg {
    Progress(String, Option<f32>),
    Done(Result<PathBuf, String>),
}

impl Default for App {
    fn default() -> Self {
        let out_dir = std::env::var_os("USERPROFILE")
            .map(|h| PathBuf::from(h).join("Music").join("Karaoke"))
            .unwrap_or_else(|| PathBuf::from("karaoke_out"));
        Self {
            opts: Options { out_dir, ..Options::default() },
            auto_play: true,
            rx: None,
            task: Task::default(),
            log: Vec::new(),
            warnings: Vec::new(),
            error: None,
            player: None,
            has_video: false,
            show_lyrics: true,
            seek_drag: None,
            burn_lyrics: true,
            lyrics_editor: None,
            export_rx: None,
            export_task: Task::default(),
            export_result: None,
        }
    }
}

fn fmt_time(t: f64) -> String {
    let s = t.max(0.0) as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

impl App {
    fn running(&self) -> bool {
        self.rx.is_some()
    }

    // ───────────────────────────── Traitement ─────────────────────────────

    fn start(&mut self, ctx: egui::Context) {
        self.log.clear();
        self.warnings.clear();
        self.error = None;
        self.task = Task { stage: "Démarrage".into(), progress: None };

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
        let settings = ExportSettings { tempo, semitones, burn_lyrics: self.burn_lyrics };
        let (tx, rx) = mpsc::channel();
        self.export_rx = Some(rx);
        self.export_result = None;
        self.export_task = Task { stage: "Préparation".into(), progress: None };
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

    fn poll_export(&mut self) {
        let Some(rx) = &self.export_rx else { return };
        loop {
            match rx.try_recv() {
                Ok(ExportMsg::Progress(s, p)) => self.export_task = Task { stage: s, progress: p },
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
        ui.add_space(6.0);
        ui.heading(RichText::new("Karaoké").size(26.0));
        ui.label("Télécharge une vidéo YouTube, affiche les paroles et retire la voix.");
        ui.small("Aucune installation : les outils se téléchargent automatiquement au premier usage.");
        ui.separator();

        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            self.form(ui);
            ui.add_space(10.0);
            let can_start = !self.running() && !self.opts.input.trim().is_empty();
            let button = egui::Button::new(RichText::new("Créer le karaoké").size(17.0).strong())
                .min_size(egui::vec2(ui.available_width(), 38.0));
            if ui.add_enabled(can_start, button).clicked() {
                self.start(ui.ctx().clone());
            }
            if ui.button("Ouvrir un karaoké déjà créé…").clicked() {
                if let Some(p) = rfd::FileDialog::new()
                    .add_filter("Karaoké", &["mp4", "m4a"])
                    .set_directory(&self.opts.out_dir)
                    .pick_file()
                {
                    self.load(Outcome::open(&p));
                }
            }
            ui.add_space(8.0);
            self.status(ui);
            ui.add_space(6.0);
            egui::CollapsingHeader::new("Journal").show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("journal")
                    .max_height(200.0)
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        for l in &self.log {
                            ui.label(RichText::new(l).monospace().size(11.0));
                        }
                    });
            });
        });
    }

    fn form(&mut self, ui: &mut egui::Ui) {
        let enabled = !self.running();
        ui.add_enabled_ui(enabled, |ui| {
            ui.label(RichText::new("Vidéo YouTube ou fichier").strong());
            ui.add(
                egui::TextEdit::singleline(&mut self.opts.input)
                    .hint_text("https://www.youtube.com/watch?v=…")
                    .desired_width(f32::INFINITY),
            );
            if ui.button("Choisir un fichier…").clicked() {
                if let Some(p) = rfd::FileDialog::new()
                    .add_filter(
                        "Audio / vidéo",
                        &["mp4", "mkv", "webm", "mov", "avi", "mp3", "m4a", "wav", "flac", "ogg", "opus"],
                    )
                    .pick_file()
                {
                    self.opts.input = p.display().to_string();
                }
            }

            ui.add_space(8.0);
            ui.label(RichText::new("Suppression de la voix").strong());
            ui.radio_value(&mut self.opts.method, Method::Ia, "Qualité : IA (≈ 1 min par chanson)");
            ui.radio_value(&mut self.opts.method, Method::Spectral, "Rapide : canal central (quelques s)");
            if self.opts.method == Method::Spectral {
                ui.add(egui::Slider::new(&mut self.opts.strength, 0.0..=1.0).text("Intensité").fixed_decimals(2));
            }

            ui.add_space(8.0);
            ui.label(RichText::new("Paroles").strong());
            ui.checkbox(&mut self.opts.lyrics, "Afficher les paroles synchronisées");
            ui.add_enabled_ui(self.opts.lyrics, |ui| {
                let current = LANGUAGES
                    .iter()
                    .find(|(c, _)| *c == self.opts.language)
                    .map_or("?", |(_, n)| *n);
                egui::ComboBox::from_label("Langue")
                    .selected_text(current)
                    .show_ui(ui, |ui| {
                        for (code, name) in LANGUAGES {
                            ui.selectable_value(&mut self.opts.language, code.to_string(), name);
                        }
                    });
                ui.checkbox(&mut self.opts.lyrics_online, "Chercher les paroles exactes en ligne (LRCLIB)");
                egui::CollapsingHeader::new("Coller les paroles exactes (facultatif)").show(ui, |ui| {
                    ui.add(
                        egui::TextEdit::multiline(&mut self.opts.lyrics_text)
                            .hint_text("Une ligne par vers. Les horodatages LRC [mm:ss.xx] sont acceptés.")
                            .desired_rows(6)
                            .desired_width(f32::INFINITY),
                    );
                });
                ui.small("Le minutage de chaque mot vient de la reconnaissance vocale (Whisper, ≈ 550 Mo au premier usage).");
            });

            ui.add_space(8.0);
            ui.label(RichText::new("Options").strong());
            ui.checkbox(&mut self.opts.audio_only, "Audio seulement (pas de vidéo)");
            ui.checkbox(&mut self.auto_play, "Lancer la lecture à la fin");
            ui.checkbox(&mut self.opts.force, "Tout refaire, même si déjà traité");
            ui.horizontal(|ui| {
                ui.label("Dossier :");
                if ui.button("Changer…").clicked() {
                    if let Some(d) = rfd::FileDialog::new().set_directory(&self.opts.out_dir).pick_folder() {
                        self.opts.out_dir = d;
                    }
                }
            });
            ui.add(egui::Label::new(RichText::new(self.opts.out_dir.display().to_string()).weak().small()).truncate());
        });
    }

    fn status(&mut self, ui: &mut egui::Ui) {
        if self.running() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new(&self.task.stage).strong());
            });
            progress_bar(ui, self.task.progress);
        }
        for w in &self.warnings {
            ui.colored_label(Color32::from_rgb(0xd0, 0x8a, 0x00), format!("⚠ {w}"));
        }
        if let Some(e) = &self.error {
            ui.colored_label(ui.visuals().error_fg_color, format!("Erreur : {e}"));
        }
    }

    // ───────────────────────────── Lecteur ─────────────────────────────

    fn player_ui(&mut self, ui: &mut egui::Ui) {
        let Some(p) = &mut self.player else {
            ui.centered_and_justified(|ui| {
                ui.label(
                    RichText::new("Collez une adresse YouTube à gauche puis cliquez sur « Créer le karaoké ».\nLa vidéo s'affichera ici.")
                        .size(16.0)
                        .weak(),
                );
            });
            return;
        };

        let ctx = ui.ctx().clone();
        let duration = p.duration();
        let t = self.seek_drag.unwrap_or_else(|| p.position());
        p.update_video(&ctx, t);

        ui.label(RichText::new(&p.title).size(18.0).strong());

        // Zone d'image (proportions conservées, centrée).
        let controls_h = 150.0;
        let avail = ui.available_size();
        let aspect = p.aspect();
        let mut h = (avail.x / aspect).min((avail.y - controls_h).max(120.0));
        let w = (h * aspect).min(avail.x);
        h = w / aspect;
        let (area, resp) = ui.allocate_exact_size(egui::vec2(avail.x, h), Sense::click());
        let img = Rect::from_center_size(area.center(), egui::vec2(w, h));
        let painter = ui.painter_at(area);
        painter.rect_filled(img, 6.0, Color32::BLACK);
        match p.texture() {
            Some(tex) => {
                painter.image(tex.id(), img, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
            }
            None if !p.has_video() => {
                painter.text(
                    img.center() - egui::vec2(0.0, h * 0.2),
                    egui::Align2::CENTER_CENTER,
                    "♫",
                    egui::FontId::proportional(h * 0.25),
                    Color32::from_gray(70),
                );
            }
            None => {}
        }
        if self.show_lyrics {
            if let Some(ly) = &p.lyrics {
                player::draw_lyrics(&painter, img, ly, t);
            }
        }
        if p.original() {
            // Repère visuel : on écoute la version originale.
            let badge = Rect::from_min_size(img.min + egui::vec2(12.0, 12.0), egui::vec2(190.0, 30.0));
            painter.rect_filled(badge, 6.0, Color32::from_rgba_unmultiplied(200, 40, 40, 220));
            painter.text(
                badge.center(),
                egui::Align2::CENTER_CENTER,
                "🎤 VERSION ORIGINALE",
                egui::FontId::proportional(15.0),
                Color32::WHITE,
            );
        }
        if resp.clicked() {
            p.toggle();
        }

        ui.add_space(6.0);
        // Barre de lecture
        ui.horizontal(|ui| {
            let label = if p.is_playing() { "⏸ Pause" } else { "⏵ Lecture" };
            if ui.add(egui::Button::new(RichText::new(label).size(15.0)).min_size(egui::vec2(96.0, 28.0))).clicked() {
                p.toggle();
            }
            // Comparaison avec l'original (avec la voix), à tout moment.
            let mut original = p.original();
            let ready = p.original_available();
            let text = if original { "🎤 Voix : originale" } else { "🎤 Voix : retirée" };
            let r = ui
                .add_enabled_ui(ready, |ui| ui.toggle_value(&mut original, RichText::new(text).size(15.0)))
                .inner
                .on_hover_text("Basculer entre le karaoké et la version originale (touche V)")
                .on_disabled_hover_text("Version originale en cours de chargement… (ou introuvable)");
            if r.changed() {
                p.set_original(original);
            }
            ui.label(RichText::new(format!("{} / {}", fmt_time(t), fmt_time(duration))).monospace());
            let mut pos = t;
            ui.spacing_mut().slider_width = (ui.available_width() - 10.0).max(50.0);
            let r = ui.add(egui::Slider::new(&mut pos, 0.0..=duration.max(0.1)).show_value(false));
            if r.dragged() {
                self.seek_drag = Some(pos);
            }
            if r.drag_stopped() || (r.changed() && !r.dragged()) {
                p.seek(pos);
                self.seek_drag = None;
            }
        });

        // Tonalité, vitesse, volume
        ui.horizontal(|ui| {
            ui.spacing_mut().slider_width = 150.0;
            let mut semi = p.semitones();
            ui.label("Tonalité");
            if ui
                .add(
                    egui::Slider::new(&mut semi, player::SEMITONE_RANGE)
                        .step_by(0.5)
                        .custom_formatter(|v, _| if v == 0.0 { "0".into() } else { format!("{v:+}") })
                        .suffix(" ½ton"),
                )
                .changed()
            {
                p.set_semitones(semi);
            }
            ui.add_space(12.0);
            let mut tempo = p.tempo() * 100.0;
            ui.label("Vitesse");
            if ui
                .add(
                    egui::Slider::new(&mut tempo, 50.0..=150.0)
                        .step_by(5.0)
                        .suffix(" %"),
                )
                .changed()
            {
                p.set_tempo(tempo / 100.0);
            }
            ui.add_space(12.0);
            let mut vol = p.volume() * 100.0;
            ui.label("Volume");
            ui.spacing_mut().slider_width = 90.0;
            if ui.add(egui::Slider::new(&mut vol, 0.0..=150.0).show_value(false)).changed() {
                p.set_volume(vol / 100.0);
            }
            if ui.button("Réinitialiser").on_hover_text("Tonalité et vitesse d'origine").clicked() {
                p.set_semitones(0.0);
                p.set_tempo(1.0);
            }
        });

        // Paroles et enregistrement
        let has_lyrics = p.lyrics.is_some();
        let mut open_editor = false;
        let (mut show_lyrics, mut burn_lyrics) = (self.show_lyrics, self.burn_lyrics);
        let (exporting, has_video) = (self.export_rx.is_some(), self.has_video);
        let (mut do_export, mut do_reveal) = (false, false);
        ui.horizontal(|ui| {
            ui.add_enabled(has_lyrics, egui::Checkbox::new(&mut show_lyrics, "Paroles à l'écran"));
            if let Some(ly) = &mut p.lyrics {
                ui.label("Décalage");
                let mut off = ly.offset;
                let r = ui
                    .add(egui::DragValue::new(&mut off).range(-5.0..=5.0).speed(0.01).fixed_decimals(2).suffix(" s"))
                    .on_hover_text("Positif : les paroles s'allument plus tôt ; négatif : plus tard.");
                if r.changed() {
                    ly.offset = off;
                }
                if r.drag_stopped() || r.lost_focus() {
                    if let Some(dir) = p.path.parent() {
                        let _ = ly.save(&dir.join(pipeline::LYRICS_FILE));
                    }
                }
            } else {
                ui.small("(pas de paroles pour ce morceau)");
            }
            if ui.button("✏ Corriger les paroles…").clicked() {
                open_editor = true;
            }
            ui.separator();
            if has_video {
                ui.add_enabled(
                    has_lyrics && !exporting,
                    egui::Checkbox::new(&mut burn_lyrics, "Incruster les paroles"),
                );
            }
            do_export = ui
                .add_enabled(!exporting, egui::Button::new(RichText::new("💾 Enregistrer…").strong()))
                .on_hover_text("Enregistre le karaoké avec la tonalité et la vitesse actuelles")
                .clicked();
            do_reveal = ui.button("Afficher dans le dossier").clicked();
        });
        if do_reveal {
            let _ = media::reveal(&p.path);
        }
        self.show_lyrics = show_lyrics;
        self.burn_lyrics = burn_lyrics;
        if do_export {
            self.start_export(ctx.clone());
        }

        if open_editor {
            self.lyrics_editor = Some(self.player.as_ref().and_then(|p| p.lyrics.as_ref()).map(|l| l.to_text()).unwrap_or_default());
        }
        self.lyrics_editor_window(&ctx);

        if self.export_rx.is_some() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(&self.export_task.stage);
            });
            progress_bar(ui, self.export_task.progress);
        }
        match &self.export_result {
            Some(Ok(path)) => {
                let path = path.clone();
                ui.horizontal(|ui| {
                    ui.colored_label(Color32::from_rgb(0x2e, 0xa0, 0x43), format!("Enregistré : {}", path.display()));
                    if ui.small_button("Afficher").clicked() {
                        let _ = media::reveal(&path);
                    }
                });
            }
            Some(Err(e)) => {
                ui.colored_label(ui.visuals().error_fg_color, format!("Enregistrement impossible : {e}"));
            }
            None => {}
        }

        // Raccourcis clavier (si aucun champ de texte n'a le focus).
        let typing = ctx.memory(|m| m.focused().is_some());
        if !typing {
            let (space, left, right, voice) = ctx.input(|i| {
                (
                    i.key_pressed(egui::Key::Space),
                    i.key_pressed(egui::Key::ArrowLeft),
                    i.key_pressed(egui::Key::ArrowRight),
                    i.key_pressed(egui::Key::V),
                )
            });
            if let Some(p) = &mut self.player {
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
        }

        if self.player.as_ref().is_some_and(|p| p.is_playing()) {
            ctx.request_repaint();
        }
    }
}

impl App {
    /// Fenêtre de correction des paroles : le texte corrigé est recalé sur la
    /// reconnaissance vocale (et sur le début des vers existants si leur nombre est inchangé).
    fn lyrics_editor_window(&mut self, ctx: &egui::Context) {
        let Some(text) = &mut self.lyrics_editor else { return };
        let mut open = true;
        let mut apply = false;
        let mut cancel = false;
        egui::Window::new("Corriger les paroles")
            .open(&mut open)
            .default_size([520.0, 560.0])
            .collapsible(false)
            .show(ctx, |ui| {
                ui.label("Une ligne par vers. Corrigez les mots, ajoutez ou supprimez des lignes, ou collez les paroles exactes.");
                ui.small("Les horodatages LRC « [mm:ss.xx] » en début de ligne sont acceptés.");
                egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
                    ui.add(egui::TextEdit::multiline(text).desired_rows(18).desired_width(f32::INFINITY));
                });
                ui.horizontal(|ui| {
                    apply = ui.button(RichText::new("Appliquer").strong()).clicked();
                    cancel = ui.button("Annuler").clicked();
                });
            });
        if apply {
            if let (Some(p), Some(text)) = (&mut self.player, &self.lyrics_editor) {
                let mut lines = karaoke::lyrics::parse_text(text);
                let old = p.lyrics.clone().unwrap_or_default();
                let synced = lines.iter().any(|l| l.start.is_some());
                if !synced && lines.len() == old.lines.len() {
                    for (l, o) in lines.iter_mut().zip(&old.lines) {
                        l.start = Some(o.start());
                    }
                }
                let dir = p.path.parent().map(PathBuf::from).unwrap_or_default();
                let (language, recognized) =
                    karaoke::lyrics::read_whisper(&dir.join(karaoke::lyrics::WHISPER_JSON), &old.language)
                        .unwrap_or_default();
                let new = karaoke::lyrics::Lyrics {
                    language: if language.is_empty() { old.language } else { language },
                    lines: karaoke::align::align(&lines, &recognized, p.duration()),
                    offset: old.offset,
                };
                let _ = new.save(&dir.join(pipeline::LYRICS_FILE));
                p.lyrics = Some(new);
                self.show_lyrics = true;
            }
        }
        if apply || cancel || !open {
            self.lyrics_editor = None;
        }
    }
}

fn progress_bar(ui: &mut egui::Ui, p: Option<f32>) {
    let bar = match p {
        Some(p) => egui::ProgressBar::new(p).show_percentage(),
        None => egui::ProgressBar::new(0.0).animate(true),
    };
    ui.add(bar);
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll();
        self.poll_export();
        let dropped: Vec<PathBuf> = ui.ctx().input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| p.is_file()).collect());
        if let Some(path) = dropped.into_iter().next() {
            if !self.running() {
                self.open_path(path);
            }
        }

        egui::Panel::left("reglages")
            .resizable(true)
            .default_size(360.0)
            .min_size(300.0)
            .show(ui, |ui| self.side_panel(ui));
        egui::CentralPanel::default().show(ui, |ui| self.player_ui(ui));
    }
}
