//! Onglets Bibliothèque et Historique du panneau gauche, réglages enregistrés et
//! déplacement des anciens karaokés vers `Documents\MagicMus`.

use crate::theme::{self as th, W};
use crate::App;
use chrono::{Datelike, Local, TimeZone};
use eframe::egui::{self, Align, Align2, Color32, CornerRadius, CursorIcon, Layout, Pos2, Rect, Sense, Stroke, Vec2};
use karaoke::library::{self, History, Library};
use karaoke::pipeline::{Method, Outcome};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Onglets du panneau gauche.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Create,
    Library,
    History,
}

/// Réglages mémorisés entre deux lancements (`reglages.json`).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub highlight: usize,
    pub lyrics_size: f32,
    pub quality: bool,
    pub language: String,
    pub lyrics: bool,
    pub lyrics_online: bool,
    pub auto_play: bool,
    pub burn_lyrics: bool,
    pub out_dir: Option<PathBuf>,
    /// L'utilisateur a refusé de déplacer ses anciens karaokés.
    pub migration_dismissed: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            highlight: 0,
            lyrics_size: 36.0,
            quality: true,
            language: "auto".into(),
            lyrics: true,
            lyrics_online: true,
            auto_play: true,
            burn_lyrics: true,
            out_dir: None,
            migration_dismissed: false,
        }
    }
}

const SETTINGS_FILE: &str = "reglages.json";

/// Données enregistrées dans `Documents\MagicMus`.
pub struct Store {
    pub dir: PathBuf,
    pub library: Library,
    pub history: History,
    saved_settings: Option<Settings>,
}

impl Store {
    pub fn open() -> (Self, Settings) {
        let dir = library::data_dir();
        let _ = std::fs::create_dir_all(library::songs_dir());
        let mut history = History::load(&dir);
        // Chansons présentes dans le dossier mais inconnues de l'historique.
        if history.scan(&library::songs_dir()) > 0 {
            let _ = history.save(&dir);
        }
        let settings: Settings = library::load_json(&dir.join(SETTINGS_FILE));
        (Self { library: Library::load(&dir), history, saved_settings: Some(settings.clone()), dir }, settings)
    }

    pub fn save_library(&self) {
        let _ = self.library.save(&self.dir);
    }

    pub fn save_history(&self) {
        let _ = self.history.save(&self.dir);
    }

    /// Enregistre les réglages s'ils ont changé.
    pub fn save_settings(&mut self, s: &Settings) {
        if self.saved_settings.as_ref() != Some(s) {
            let _ = library::save_json(&self.dir.join(SETTINGS_FILE), s);
            self.saved_settings = Some(s.clone());
        }
    }
}

/// État d'affichage des onglets.
#[derive(Default)]
pub struct LibUi {
    pub tab: Tab,
    pub filter: String,
    pub open: HashSet<u64>,
    pub renaming: Option<(u64, String)>,
    pub clear_confirm: bool,
    /// Anciens karaokés proposés au déplacement.
    pub migrate: Option<(PathBuf, Vec<PathBuf>)>,
}

/// Actions demandées pendant l'affichage, appliquées ensuite.
enum Act {
    Open(PathBuf),
    Reveal(PathBuf),
    AddTo(PathBuf, String, Option<u64>),
    RemoveSong(PathBuf),
    MoveSong(PathBuf, Option<u64>),
    NewFolder(Option<u64>),
    StartRename(u64),
    Rename(u64, String),
    DeleteFolder(u64),
    MoveFolder(u64, Option<u64>),
    Toggle(u64),
    ForgetHistory(PathBuf),
    ClearHistory,
}

/// Élément glissé.
#[derive(Clone)]
enum Drag {
    Song(PathBuf),
    Folder(u64),
}

impl Settings {
    pub fn method(&self) -> Method {
        if self.quality { Method::Ia } else { Method::Spectral }
    }
}

// ───────────────────────────── Onglets ─────────────────────────────

/// Barre d'onglets en pastilles (onglet actif noir prune).
pub fn tabs(ui: &mut egui::Ui, tab: &mut Tab) {
    let items = [(Tab::Create, "Créer"), (Tab::Library, "Bibliothèque"), (Tab::History, "Historique")];
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 38.0), Sense::hover());
    let p = ui.painter();
    p.rect(rect, CornerRadius::same(255), th::PANEL, Stroke::new(1.0, th::BORDER_INPUT), egui::StrokeKind::Inside);
    let w = (width - 8.0) / items.len() as f32;
    for (i, (t, label)) in items.iter().enumerate() {
        let r = Rect::from_min_size(rect.min + Vec2::new(4.0 + i as f32 * w, 4.0), Vec2::new(w, 30.0));
        let resp = ui.interact(r, ui.scope_id().with(("onglet", i)), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
        let sel = *tab == *t;
        if sel {
            ui.painter().rect_filled(r, CornerRadius::same(255), th::INK);
        }
        let c = if sel { th::CREAM } else if resp.hovered() { th::ROSE } else { th::MUTED };
        ui.painter().text(r.center(), Align2::CENTER_CENTER, *label, th::font(13.0, W::Bold), c);
        if resp.clicked() {
            *tab = *t;
        }
    }
}

// ───────────────────────────── Dessins ─────────────────────────────

fn folder_icon(p: &egui::Painter, c: Pos2, color: Color32) {
    let body = Rect::from_center_size(c + Vec2::new(0.0, 1.5), Vec2::new(18.0, 13.0));
    let tab = Rect::from_min_size(body.min - Vec2::new(0.0, 3.0), Vec2::new(8.0, 5.0));
    p.rect_filled(tab, CornerRadius::same(2), color);
    p.rect_filled(body, CornerRadius::same(3), color);
}

fn chevron(p: &egui::Painter, c: Pos2, open: bool, color: Color32) {
    let pts = if open {
        vec![c + Vec2::new(-4.0, -2.0), c + Vec2::new(0.0, 2.5), c + Vec2::new(4.0, -2.0)]
    } else {
        vec![c + Vec2::new(-2.0, -4.0), c + Vec2::new(2.5, 0.0), c + Vec2::new(-2.0, 4.0)]
    };
    p.line(pts, Stroke::new(1.8, color));
}

/// Étoile à cinq branches (pleine ou en contour).
pub fn star(p: &egui::Painter, c: Pos2, r: f32, filled: bool, color: Color32) {
    let pts: Vec<Pos2> = (0..10)
        .map(|i| {
            let a = -std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::PI / 5.0;
            let rr = if i % 2 == 0 { r } else { r * 0.45 };
            c + Vec2::angled(a) * rr
        })
        .collect();
    if filled {
        let mut mesh = egui::Mesh::default();
        mesh.colored_vertex(c, color);
        for &q in &pts {
            mesh.colored_vertex(q, color);
        }
        for i in 1..=10u32 {
            mesh.add_triangle(0, i, if i == 10 { 1 } else { i + 1 });
        }
        p.add(egui::Shape::mesh(mesh));
    } else {
        let mut closed = pts.clone();
        closed.push(pts[0]);
        p.line(closed, Stroke::new(1.5, color));
    }
}

/// Bouton étoile (ajouter / retirer de la bibliothèque).
fn star_button(ui: &mut egui::Ui, r: Rect, on: bool, id: egui::Id) -> egui::Response {
    let resp = ui.interact(r, id, Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
    let color = if on || resp.hovered() { th::ROSE } else { th::CHECK_BORDER };
    star(ui.painter(), r.center(), 8.0, on, color);
    resp.on_hover_text(if on { "Dans la bibliothèque (cliquer pour retirer)" } else { "Ajouter à la bibliothèque" })
}

/// Ligne de chanson : vignette, titre, sous-titre. Renvoie la réponse (clic = ouvrir).
fn song_row(
    ui: &mut egui::Ui,
    logo: &egui::TextureHandle,
    title: &str,
    subtitle: &str,
    indent: f32,
    current: bool,
    missing: bool,
    right_space: f32,
) -> egui::Response {
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 46.0), Sense::click());
    let p = ui.painter();
    if current {
        p.rect(rect, CornerRadius::same(12), th::ROSE_CARD, Stroke::new(1.0, th::ROSE), egui::StrokeKind::Inside);
    } else if resp.hovered() && !missing {
        p.rect_filled(rect, CornerRadius::same(12), th::ROSE_CARD);
    }
    let thumb = Pos2::new(rect.left() + indent + 20.0, rect.center().y);
    p.circle_filled(thumb, 16.0, th::ROSE_SOFT);
    th::paint_logo(p, logo, thumb + Vec2::new(0.0, 1.5), 26.0);
    let x = thumb.x + 24.0;
    let max_w = (rect.right() - right_space - x).max(40.0);
    let alpha = if missing { 0.45 } else { 1.0 };
    let tg = p.layout(title.to_string(), th::font(14.0, W::Semi), th::INK, max_w);
    let tg = if tg.rows.len() > 1 {
        // une seule ligne, tronquée
        let mut job = egui::text::LayoutJob::simple_singleline(title.to_string(), th::font(14.0, W::Semi), th::INK);
        job.wrap = egui::text::TextWrapping::truncate_at_width(max_w);
        p.layout_job(job)
    } else {
        tg
    };
    p.galley_with_override_text_color(Pos2::new(x, rect.top() + 6.0), tg, th::INK.gamma_multiply(alpha));
    let mut job = egui::text::LayoutJob::simple_singleline(subtitle.to_string(), th::font(12.0, W::Medium), th::FAINT);
    job.wrap = egui::text::TextWrapping::truncate_at_width(max_w);
    p.galley(Pos2::new(x, rect.top() + 26.0), p.layout_job(job), th::FAINT);
    if missing { resp } else { resp.on_hover_cursor(CursorIcon::PointingHand) }
}

/// Date lisible en français (« 9 nov. 2026 », avec l'heure si demandé).
pub fn fmt_date(t: i64, with_time: bool) -> String {
    let Some(d) = Local.timestamp_opt(t, 0).single() else { return String::new() };
    const MOIS: [&str; 12] = ["janv.", "févr.", "mars", "avr.", "mai", "juin", "juil.", "août", "sept.", "oct.", "nov.", "déc."];
    let mut s = format!("{} {} {}", d.day(), MOIS[d.month0() as usize], d.year());
    if with_time {
        s.push_str(&d.format(" à %H:%M").to_string());
    }
    s
}

/// Intitulé du jour (« Aujourd'hui », « Hier », « lundi 3 novembre »).
fn day_label(t: i64) -> String {
    let Some(d) = Local.timestamp_opt(t, 0).single() else { return String::new() };
    let today = Local::now().date_naive();
    let day = d.date_naive();
    if day == today {
        return "Aujourd'hui".into();
    }
    if Some(day) == today.pred_opt() {
        return "Hier".into();
    }
    const JOURS: [&str; 7] = ["lundi", "mardi", "mercredi", "jeudi", "vendredi", "samedi", "dimanche"];
    const MOIS: [&str; 12] = [
        "janvier", "février", "mars", "avril", "mai", "juin", "juillet", "août", "septembre", "octobre", "novembre", "décembre",
    ];
    let mut s = format!("{} {} {}", JOURS[day.weekday().num_days_from_monday() as usize], day.day(), MOIS[day.month0() as usize]);
    if day.year() != today.year() {
        s.push_str(&format!(" {}", day.year()));
    }
    s
}

fn time_of(t: i64) -> String {
    Local.timestamp_opt(t, 0).single().map(|d| d.format("%H:%M").to_string()).unwrap_or_default()
}

/// Menu « Ranger dans » : racine et tous les dossiers.
fn folder_menu(ui: &mut egui::Ui, lib: &Library, current: Option<u64>, exclude: Option<u64>) -> Option<Option<u64>> {
    let mut chosen = None;
    if ui.add_enabled(current.is_some(), egui::Button::new("Racine de la bibliothèque")).clicked() {
        chosen = Some(None);
    }
    for (depth, f) in lib.tree() {
        if exclude.is_some_and(|x| x == f.id || lib.is_descendant(f.id, x)) {
            continue;
        }
        let label = format!("{}{}", "   ".repeat(depth), f.name);
        if ui.add_enabled(current != Some(f.id), egui::Button::new(label)).clicked() {
            chosen = Some(Some(f.id));
        }
    }
    chosen
}

// ───────────────────────────── Panneaux ─────────────────────────────

impl App {
    /// Onglet Bibliothèque.
    pub(crate) fn library_panel(&mut self, ui: &mut egui::Ui) {
        let mut acts = Vec::new();
        let lib = &self.store.library;
        let current = self.player.as_ref().map(|p| p.path.clone());

        ui.horizontal(|ui| {
            th::section_label(ui, "Bibliothèque", th::FAINT);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if th::link(ui, "+ Nouveau dossier", th::ROSE).clicked() {
                    acts.push(Act::NewFolder(None));
                }
            });
        });
        ui.add(
            egui::TextEdit::singleline(&mut self.lib_ui.filter)
                .hint_text("Rechercher une chanson…")
                .font(th::font(13.0, W::Medium))
                .margin(egui::Margin::symmetric(12, 9))
                .desired_width(f32::INFINITY),
        );

        if lib.songs.is_empty() && lib.folders.is_empty() {
            ui.add_space(20.0);
            ui.vertical_centered(|ui| {
                th::logo_disc(ui, &self.logo, 90.0, 78.0, th::ROSE_SOFT, 2.0);
                ui.add(
                    egui::Label::new(th::t(
                        "Votre bibliothèque est vide. Ajoutez une chanson avec l'étoile, depuis le lecteur ou l'historique, puis rangez-la dans des dossiers.",
                        13.0,
                        W::Medium,
                        th::MUTED,
                    ))
                    .wrap()
                    .halign(Align::Center),
                );
            });
        } else if !self.lib_ui.filter.trim().is_empty() {
            // Recherche : liste à plat avec le dossier en sous-titre.
            let q = self.lib_ui.filter.trim().to_lowercase();
            let mut found: Vec<_> = lib.songs.iter().filter(|s| s.title.to_lowercase().contains(&q)).collect();
            found.sort_by_key(|s| s.title.to_lowercase());
            if found.is_empty() {
                ui.label(th::t("Aucune chanson trouvée.", 13.0, W::Medium, th::FAINT));
            }
            for s in found {
                let sub = s.folder.map_or("Racine".to_string(), |f| lib.folder_path(f));
                let missing = !s.path.is_file();
                let r = song_row(ui, &self.logo, &s.title, &sub, 0.0, current.as_ref() == Some(&s.path), missing, 8.0);
                if r.clicked() && !missing {
                    acts.push(Act::Open(s.path.clone()));
                }
                song_menu(&r, lib, &s.path, s.folder, &mut acts);
            }
        } else {
            // Racine : cible de dépôt.
            let root = ui.allocate_response(Vec2::new(ui.available_width(), 6.0), Sense::hover());
            let hovering = root.dnd_hover_payload::<Drag>().is_some();
            if hovering {
                ui.painter().hline(root.rect.x_range(), root.rect.center().y, Stroke::new(2.0, th::ROSE));
            }
            if let Some(d) = root.dnd_release_payload::<Drag>() {
                acts.push(match &*d {
                    Drag::Song(p) => Act::MoveSong(p.clone(), None),
                    Drag::Folder(f) => Act::MoveFolder(*f, None),
                });
            }
            self.folder_level(ui, None, 0, current.as_deref(), &mut acts);
            if self.store.library.songs.is_empty() {
                ui.label(th::t("Ajoutez des chansons avec l'étoile, depuis le lecteur ou l'historique.", 12.0, W::Medium, th::FAINT));
            } else {
                ui.add_space(6.0);
                ui.label(th::t("Glissez une chanson sur un dossier pour la ranger · clic droit : plus d'options", 11.0, W::Medium, th::FAINT));
            }
        }
        self.apply(acts);
    }

    /// Dossiers puis chansons d'un niveau de l'arborescence.
    fn folder_level(&mut self, ui: &mut egui::Ui, parent: Option<u64>, depth: usize, current: Option<&Path>, acts: &mut Vec<Act>) {
        let indent = depth as f32 * 18.0;
        let folders: Vec<_> = self.store.library.children(parent).into_iter().cloned().collect();
        for f in folders {
            let open = self.lib_ui.open.contains(&f.id);
            let count = self.store.library.count_in(f.id);
            let renaming = self.lib_ui.renaming.as_ref().is_some_and(|r| r.0 == f.id);

            let resp = if renaming {
                let mut done = None;
                ui.horizontal(|ui| {
                    ui.add_space(indent + 22.0);
                    let text = &mut self.lib_ui.renaming.as_mut().unwrap().1;
                    let r = ui.add(
                        egui::TextEdit::singleline(text)
                            .font(th::font(14.0, W::Bold))
                            .margin(egui::Margin::symmetric(8, 6))
                            .desired_width(ui.available_width()),
                    );
                    r.request_focus();
                    if r.lost_focus() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        done = Some(text.clone());
                    }
                    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        done = Some(String::new());
                    }
                });
                if let Some(name) = done {
                    acts.push(Act::Rename(f.id, name));
                }
                None
            } else {
                let w = ui.available_width();
                let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 34.0), Sense::click_and_drag());
                let hover_drop = resp.dnd_hover_payload::<Drag>().is_some();
                let p = ui.painter();
                if hover_drop {
                    p.rect(rect, CornerRadius::same(10), th::ROSE_CARD, Stroke::new(1.5, th::ROSE), egui::StrokeKind::Inside);
                } else if resp.hovered() {
                    p.rect_filled(rect, CornerRadius::same(10), th::ROSE_CARD);
                }
                chevron(p, Pos2::new(rect.left() + indent + 9.0, rect.center().y), open, th::FAINT);
                folder_icon(p, Pos2::new(rect.left() + indent + 30.0, rect.center().y), th::ROSE_LIGHT);
                let mut job = egui::text::LayoutJob::simple_singleline(f.name.clone(), th::font(14.0, W::Bold), th::INK);
                job.wrap = egui::text::TextWrapping::truncate_at_width((w - indent - 90.0).max(30.0));
                p.galley(Pos2::new(rect.left() + indent + 46.0, rect.center().y - 9.0), p.layout_job(job), th::INK);
                p.text(Pos2::new(rect.right() - 8.0, rect.center().y), Align2::RIGHT_CENTER, count.to_string(), th::font(12.0, W::Bold), th::FAINT);
                if resp.clicked() {
                    acts.push(Act::Toggle(f.id));
                }
                if resp.drag_started() {
                    egui::DragAndDrop::set_payload(ui.ctx(), Drag::Folder(f.id));
                }
                if let Some(d) = resp.dnd_release_payload::<Drag>() {
                    acts.push(match &*d {
                        Drag::Song(p) => Act::MoveSong(p.clone(), Some(f.id)),
                        Drag::Folder(x) => Act::MoveFolder(*x, Some(f.id)),
                    });
                }
                Some(resp.on_hover_cursor(CursorIcon::PointingHand))
            };
            if let Some(resp) = resp {
                let lib = &self.store.library;
                resp.context_menu(|ui| {
                    if ui.button("Nouveau sous-dossier").clicked() {
                        acts.push(Act::NewFolder(Some(f.id)));
                    }
                    if ui.button("Renommer").clicked() {
                        acts.push(Act::StartRename(f.id));
                    }
                    ui.menu_button("Déplacer vers", |ui| {
                        if let Some(dest) = folder_menu(ui, lib, f.parent, Some(f.id)) {
                            acts.push(Act::MoveFolder(f.id, dest));
                        }
                    });
                    ui.separator();
                    if ui.button("Supprimer le dossier (les chansons sont conservées)").clicked() {
                        acts.push(Act::DeleteFolder(f.id));
                    }
                });
            }
            if open {
                self.folder_level(ui, Some(f.id), depth + 1, current, acts);
            }
        }

        let songs: Vec<_> = self.store.library.songs_in(parent).into_iter().cloned().collect();
        for s in songs {
            let missing = !s.path.is_file();
            let sub = if missing { "Fichier introuvable".to_string() } else { format!("Ajoutée le {}", fmt_date(s.added, false)) };
            let id = ui.scope_id().with(("chanson", &s.path));
            let resp = ui
                .dnd_drag_source(id, Drag::Song(s.path.clone()), |ui| {
                    song_row(ui, &self.logo, &s.title, &sub, indent + if depth > 0 { 4.0 } else { 0.0 }, current == Some(s.path.as_path()), missing, 8.0)
                })
                .inner;
            if resp.clicked() && !missing {
                acts.push(Act::Open(s.path.clone()));
            }
            song_menu(&resp, &self.store.library, &s.path, s.folder, acts);
        }
    }

    /// Onglet Historique.
    pub(crate) fn history_panel(&mut self, ui: &mut egui::Ui) {
        let mut acts = Vec::new();
        let current = self.player.as_ref().map(|p| p.path.clone());
        ui.horizontal(|ui| {
            th::section_label(ui, "Historique", th::FAINT);
            if !self.store.history.entries.is_empty() {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if self.lib_ui.clear_confirm {
                        if th::link(ui, "Annuler", th::MUTED).clicked() {
                            self.lib_ui.clear_confirm = false;
                        }
                        if th::link(ui, "Confirmer l'effacement", th::ROSE).clicked() {
                            acts.push(Act::ClearHistory);
                        }
                    } else if th::link(ui, "Tout effacer", th::MUTED)
                        .on_hover_text("Vide la liste (les fichiers sont conservés)")
                        .clicked()
                    {
                        self.lib_ui.clear_confirm = true;
                    }
                });
            }
        });

        let entries: Vec<_> = self.store.history.sorted().into_iter().cloned().collect();
        if entries.is_empty() {
            ui.add_space(20.0);
            ui.vertical_centered(|ui| {
                th::logo_disc(ui, &self.logo, 90.0, 78.0, th::ROSE_SOFT, 2.0);
                ui.add(
                    egui::Label::new(th::t("Les chansons que vous créez et écoutez apparaîtront ici.", 13.0, W::Medium, th::MUTED))
                        .wrap()
                        .halign(Align::Center),
                );
            });
        }
        let mut last_day = String::new();
        for e in entries {
            let day = day_label(e.last_activity());
            if day != last_day {
                ui.add_space(6.0);
                ui.label(th::t(&day, 12.0, W::Black, th::ROSE));
                last_day = day;
            }
            let missing = !e.path.is_file();
            let sub = if missing {
                "Fichier introuvable".to_string()
            } else {
                match e.last_played.filter(|&p| p >= e.created) {
                    Some(p) => format!(
                        "Écoutée à {} · {} écoute{}",
                        time_of(p),
                        e.plays,
                        if e.plays > 1 { "s" } else { "" }
                    ),
                    None => format!("Créée à {}", time_of(e.created)),
                }
            };
            let resp = song_row(ui, &self.logo, &e.title, &sub, 0.0, current.as_ref() == Some(&e.path), missing, 34.0);
            let in_lib = self.store.library.contains(&e.path);
            if !missing {
                let sr = Rect::from_center_size(Pos2::new(resp.rect.right() - 18.0, resp.rect.center().y), Vec2::splat(26.0));
                if star_button(ui, sr, in_lib, ui.scope_id().with(("etoile", &e.path))).clicked() {
                    acts.push(if in_lib { Act::RemoveSong(e.path.clone()) } else { Act::AddTo(e.path.clone(), e.title.clone(), None) });
                } else if resp.clicked() {
                    acts.push(Act::Open(e.path.clone()));
                }
            }
            let lib = &self.store.library;
            resp.context_menu(|ui| {
                if !missing {
                    ui.menu_button(if in_lib { "Ranger dans" } else { "Ajouter à la bibliothèque" }, |ui| {
                        let cur = lib.songs.iter().find(|s| s.path == e.path).map(|s| s.folder);
                        if let Some(dest) = folder_menu(ui, lib, cur.flatten().or(if in_lib { None } else { Some(u64::MAX) }), None) {
                            acts.push(Act::AddTo(e.path.clone(), e.title.clone(), dest));
                        }
                    });
                    if ui.button("Afficher dans le dossier").clicked() {
                        acts.push(Act::Reveal(e.path.clone()));
                    }
                }
                if ui.button("Retirer de l'historique").clicked() {
                    acts.push(Act::ForgetHistory(e.path.clone()));
                }
            });
        }
        self.apply(acts);
    }

    fn apply(&mut self, acts: Vec<Act>) {
        self.apply_acts(acts);
    }
}

/// Bouton « Bibliothèque » de l'en-tête du lecteur : ajoute la chanson ouverte (ou
/// la range dans un autre dossier). Fonction libre : n'emprunte que la bibliothèque.
pub(crate) fn library_button(ui: &mut egui::Ui, store: &mut Store, lib_ui: &mut LibUi, path: &Path, title: &str) {
    let in_lib = store.library.contains(path);
    {
        let label = if in_lib { "Dans la bibliothèque" } else { "Ajouter à la bibliothèque" };
        let w = ui.painter().layout_no_wrap(label.into(), th::font(13.0, W::Bold), th::INK).size().x + 54.0;
        let resp = th::secondary_sized(ui, "", Vec2::new(w, 34.0), 10);
        let p = ui.painter();
        star(p, Pos2::new(resp.rect.left() + 18.0, resp.rect.center().y), 7.0, in_lib, th::ROSE);
        p.text(
            Pos2::new(resp.rect.left() + 32.0, resp.rect.center().y),
            Align2::LEFT_CENTER,
            label,
            th::font(13.0, W::Bold),
            if resp.hovered() || in_lib { th::ROSE } else { th::INK },
        );
        let mut add: Option<Option<u64>> = None;
        let mut remove = false;
        let lib = &store.library;
        egui::Popup::menu(&resp).show(|ui| {
            ui.label(th::t(if in_lib { "Ranger dans :" } else { "Ajouter dans :" }, 12.0, W::Bold, th::FAINT));
            let cur = lib.songs.iter().find(|s| s.path == path).map(|s| s.folder);
            add = folder_menu(ui, lib, cur.flatten().or(if in_lib { None } else { Some(u64::MAX) }), None);
            if in_lib {
                ui.separator();
                remove = ui.button("Retirer de la bibliothèque").clicked();
            }
        });
        if let Some(folder) = add {
            store.library.add(path, title, folder);
            if let Some(f) = folder {
                lib_ui.open.insert(f);
            }
            store.save_library();
        }
        if remove {
            store.library.remove(path);
            store.save_library();
        }
    }
}

impl App {
    fn apply_acts(&mut self, acts: Vec<Act>) {
        let (mut lib_changed, mut hist_changed) = (false, false);
        for a in acts {
            match a {
                Act::Open(p) => self.load(Outcome::open(&p)),
                Act::Reveal(p) => {
                    let _ = karaoke::media::reveal(&p);
                }
                Act::AddTo(p, title, folder) => {
                    let title = if title.is_empty() { library::song_title(&p) } else { title };
                    self.store.library.add(&p, &title, folder);
                    if let Some(f) = folder {
                        self.lib_ui.open.insert(f);
                    }
                    lib_changed = true;
                }
                Act::RemoveSong(p) => {
                    self.store.library.remove(&p);
                    lib_changed = true;
                }
                Act::MoveSong(p, folder) => {
                    self.store.library.move_song(&p, folder);
                    if let Some(f) = folder {
                        self.lib_ui.open.insert(f);
                    }
                    lib_changed = true;
                }
                Act::NewFolder(parent) => {
                    let id = self.store.library.create_folder("Nouveau dossier", parent);
                    if let Some(p) = parent {
                        self.lib_ui.open.insert(p);
                    }
                    self.lib_ui.renaming = Some((id, "Nouveau dossier".into()));
                    lib_changed = true;
                }
                Act::StartRename(id) => {
                    let name = self.store.library.folders.iter().find(|f| f.id == id).map(|f| f.name.clone()).unwrap_or_default();
                    self.lib_ui.renaming = Some((id, name));
                }
                Act::Rename(id, name) => {
                    self.store.library.rename_folder(id, &name);
                    self.lib_ui.renaming = None;
                    lib_changed = true;
                }
                Act::DeleteFolder(id) => {
                    self.store.library.delete_folder(id);
                    lib_changed = true;
                }
                Act::MoveFolder(id, parent) => {
                    if self.store.library.move_folder(id, parent) {
                        if let Some(p) = parent {
                            self.lib_ui.open.insert(p);
                        }
                        lib_changed = true;
                    }
                }
                Act::Toggle(id) => {
                    if !self.lib_ui.open.remove(&id) {
                        self.lib_ui.open.insert(id);
                    }
                }
                Act::ForgetHistory(p) => {
                    self.store.history.remove(&p);
                    hist_changed = true;
                }
                Act::ClearHistory => {
                    self.store.history.clear();
                    self.lib_ui.clear_confirm = false;
                    hist_changed = true;
                }
            }
        }
        if lib_changed {
            self.store.save_library();
        }
        if hist_changed {
            self.store.save_history();
        }
    }

    // ───────────────────────────── Anciens karaokés ─────────────────────────────

    /// Propose de déplacer les karaokés de l'ancien dossier (`Musique\Karaoke`).
    pub(crate) fn detect_old_songs(&mut self, settings: &Settings) {
        if settings.migration_dismissed {
            return;
        }
        let old = library::user_folder("Music").join("Karaoke");
        let dirs = library::song_dirs(&old);
        if !dirs.is_empty() && old != library::songs_dir() {
            self.lib_ui.migrate = Some((old, dirs));
        }
    }

    /// Bandeau de déplacement des anciens karaokés.
    pub(crate) fn migration_banner(&mut self, ui: &mut egui::Ui) {
        let Some((old, dirs)) = self.lib_ui.migrate.clone() else { return };
        let (mut go, mut later) = (false, false);
        th::card(14, egui::Margin::symmetric(16, 12)).fill(th::ROSE_CARD).stroke(Stroke::new(1.0, th::ROSE_LIGHT)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.set_width((ui.available_width() - 330.0).max(200.0));
                    ui.label(th::t(format!("{} karaoké(s) dans l'ancien dossier", dirs.len()), 14.0, W::Bold, th::INK));
                    ui.add(
                        egui::Label::new(th::t(
                            format!("{} → {}", old.display(), library::songs_dir().display()),
                            12.0,
                            W::Medium,
                            th::MUTED,
                        ))
                        .truncate(),
                    );
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    later = th::secondary(ui, "Plus tard").on_hover_text("Ne plus proposer").clicked();
                    go = th::primary_fit(ui, "Déplacer dans Documents", 13.0, 34.0).clicked();
                });
            });
        });
        ui.add_space(10.0);
        if go {
            // Le lecteur peut garder un fichier ouvert : on le ferme avant de déplacer.
            if self.player.as_ref().is_some_and(|p| p.path.starts_with(&old)) {
                self.player = None;
            }
            match library::move_song_dirs(&dirs, &library::songs_dir()) {
                Ok(n) => {
                    let added = self.store.history.scan(&library::songs_dir());
                    self.store.save_history();
                    self.log.push(format!("{n} karaoké(s) déplacé(s), {added} ajouté(s) à l'historique"));
                    self.lib_ui.tab = Tab::History;
                }
                Err(e) => self.error = Some(format!("Déplacement impossible : {e:#}")),
            }
            self.lib_ui.migrate = None;
        }
        if later {
            self.settings.migration_dismissed = true;
            self.lib_ui.migrate = None;
        }
    }
}

/// Menu contextuel d'une chanson de la bibliothèque.
fn song_menu(resp: &egui::Response, lib: &Library, path: &Path, folder: Option<u64>, acts: &mut Vec<Act>) {
    resp.context_menu(|ui| {
        ui.menu_button("Déplacer vers", |ui| {
            if let Some(dest) = folder_menu(ui, lib, folder, None) {
                acts.push(Act::MoveSong(path.to_path_buf(), dest));
            }
        });
        if ui.button("Afficher dans le dossier").clicked() {
            acts.push(Act::Reveal(path.to_path_buf()));
        }
        ui.separator();
        if ui.button("Retirer de la bibliothèque").clicked() {
            acts.push(Act::RemoveSong(path.to_path_buf()));
        }
    });
}
