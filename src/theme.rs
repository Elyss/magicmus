//! Thème « MagicMus » : couleurs, polices et composants dessinés à la main
//! (d'après la maquette Claude Design : tons crème et rose, boutons noir prune,
//! titres en DM Serif Display, texte en Nunito).

use eframe::egui::{
    self, Align2, Color32, CornerRadius, CursorIcon, FontData, FontDefinitions, FontFamily, FontId, Frame,
    Margin, Mesh, Pos2, Rect, Response, RichText, Sense, Shape, Stroke, StrokeKind, TextureHandle, Ui, Vec2,
};
use std::sync::Arc;

// ───────────────────────────── Couleurs ─────────────────────────────

const fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

pub const BG: Color32 = rgb(0xf8f3ee); // fond de la zone principale
pub const PANEL: Color32 = rgb(0xfffcf9); // panneaux et cartes
pub const BORDER: Color32 = rgb(0xece3dc);
pub const BORDER_INPUT: Color32 = rgb(0xe4dad3);
pub const INK: Color32 = rgb(0x241c25); // texte principal
pub const MUTED: Color32 = rgb(0x75686f);
pub const FAINT: Color32 = rgb(0x9a8b93);
pub const ROSE: Color32 = rgb(0xb4587f); // accent
pub const ROSE_LIGHT: Color32 = rgb(0xc97a9c);
pub const ROSE_SOFT: Color32 = rgb(0xf6e7ee);
pub const ROSE_CARD: Color32 = rgb(0xf8ecf1);
pub const TRACK: Color32 = rgb(0xefe6e9);
pub const SWITCH_OFF: Color32 = rgb(0xddd3d8);
pub const CHECK_BORDER: Color32 = rgb(0xcfc3c9);
pub const STEP_BORDER: Color32 = rgb(0xd9cdd3);
pub const STEP_TODO: Color32 = rgb(0xa89aa1);
pub const PLUM: Color32 = rgb(0x2b2130); // boutons principaux
pub const PLUM_HOVER: Color32 = rgb(0x3a2d40);
pub const CREAM: Color32 = rgb(0xfbf6f1);
pub const SEGMENT_BG: Color32 = rgb(0xf3ebef);
pub const VIDEO_BG: Color32 = rgb(0x1d1820);

/// Couleurs proposées pour les mots chantés.
pub const HIGHLIGHTS: [(Color32, &str); 4] = [
    (rgb(0xf7a6c9), "Rose"),
    (rgb(0xffd700), "Or"),
    (rgb(0xf5e6c8), "Crème"),
    (rgb(0xb9a7ff), "Lavande"),
];

// ───────────────────────────── Polices ─────────────────────────────

/// Graisses et familles disponibles.
#[derive(Clone, Copy)]
pub enum W {
    Medium,
    Semi,
    Bold,
    Black,
    Serif,
    SerifItalic,
}

pub fn family(w: W) -> FontFamily {
    match w {
        W::Medium => FontFamily::Proportional,
        W::Semi => FontFamily::Name("semibold".into()),
        W::Bold => FontFamily::Name("bold".into()),
        W::Black => FontFamily::Name("black".into()),
        W::Serif => FontFamily::Name("serif".into()),
        W::SerifItalic => FontFamily::Name("serif-italic".into()),
    }
}

pub fn font(size: f32, w: W) -> FontId {
    FontId::new(size, family(w))
}

/// Texte avec taille, graisse et couleur.
pub fn t(text: impl Into<String>, size: f32, w: W, color: Color32) -> RichText {
    RichText::new(text).font(font(size, w)).color(color)
}

/// Installe les polices (Nunito, DM Serif Display) et le style clair de l'application.
pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let files: [(&str, &'static [u8]); 6] = [
        ("Nunito-Medium", include_bytes!("../assets/fonts/Nunito-Medium.ttf")),
        ("Nunito-SemiBold", include_bytes!("../assets/fonts/Nunito-SemiBold.ttf")),
        ("Nunito-Bold", include_bytes!("../assets/fonts/Nunito-Bold.ttf")),
        ("Nunito-ExtraBold", include_bytes!("../assets/fonts/Nunito-ExtraBold.ttf")),
        ("DMSerif", include_bytes!("../assets/fonts/DMSerifDisplay-Regular.ttf")),
        ("DMSerif-Italic", include_bytes!("../assets/fonts/DMSerifDisplay-Italic.ttf")),
    ];
    for (name, bytes) in files {
        fonts.font_data.insert(name.into(), Arc::new(FontData::from_static(bytes)));
    }
    // Les polices d'egui restent en secours (symboles, emoji, alphabets non latins).
    let fallback = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    fonts.families.entry(FontFamily::Proportional).or_default().insert(0, "Nunito-Medium".into());
    for (fam, file) in [
        ("semibold", "Nunito-SemiBold"),
        ("bold", "Nunito-Bold"),
        ("black", "Nunito-ExtraBold"),
        ("serif", "DMSerif"),
        ("serif-italic", "DMSerif-Italic"),
    ] {
        let mut list = vec![file.to_string()];
        list.extend(fallback.iter().cloned());
        fonts.families.insert(FontFamily::Name(fam.into()), list);
    }
    ctx.set_fonts(fonts);

    ctx.set_theme(egui::Theme::Light);
    ctx.style_mut_of(egui::Theme::Light, |style| {
        use egui::TextStyle;
        style.text_styles.insert(TextStyle::Body, font(14.0, W::Medium));
        style.text_styles.insert(TextStyle::Button, font(13.0, W::Bold));
        style.text_styles.insert(TextStyle::Small, font(12.0, W::Medium));
        style.text_styles.insert(TextStyle::Heading, font(30.0, W::Serif));
        style.spacing.button_padding = Vec2::new(12.0, 7.0);
        style.spacing.item_spacing = Vec2::new(10.0, 8.0);
        style.spacing.interact_size.y = 30.0;

        let v = &mut style.visuals;
        v.panel_fill = BG;
        v.window_fill = PANEL;
        v.window_stroke = Stroke::NONE;
        v.window_corner_radius = CornerRadius::same(22);
        v.extreme_bg_color = Color32::WHITE; // champs de saisie
        v.faint_bg_color = BG;
        v.hyperlink_color = ROSE;
        v.selection.bg_fill = ROSE_SOFT;
        v.selection.stroke = Stroke::new(1.5, ROSE);
        v.override_text_color = None;
        v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, INK);
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
        for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
            w.corner_radius = CornerRadius::same(10);
            w.bg_fill = Color32::WHITE;
            w.weak_bg_fill = Color32::WHITE;
            w.fg_stroke = Stroke::new(1.0, INK);
            w.expansion = 0.0;
        }
        v.widgets.inactive.bg_stroke = Stroke::new(1.0, BORDER_INPUT);
        v.widgets.hovered.bg_stroke = Stroke::new(1.0, ROSE);
        v.widgets.hovered.fg_stroke = Stroke::new(1.0, ROSE);
        v.widgets.active.bg_stroke = Stroke::new(1.5, ROSE);
        v.widgets.active.fg_stroke = Stroke::new(1.0, ROSE);
        v.widgets.open.bg_stroke = Stroke::new(1.0, ROSE);
    });
}

/// Logo de l'application (512 px, intégré à l'exe).
pub fn logo(ctx: &egui::Context) -> TextureHandle {
    let img = image::load_from_memory(include_bytes!("../assets/magicmus-512.png"))
        .expect("logo intégré illisible")
        .to_rgba8();
    let size = [img.width() as usize, img.height() as usize];
    ctx.load_texture("logo", egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw()), egui::TextureOptions::LINEAR)
}

/// Icône de la fenêtre.
pub fn window_icon() -> Option<egui::IconData> {
    let img = image::load_from_memory(include_bytes!("../assets/magicmus-256.png")).ok()?.to_rgba8();
    Some(egui::IconData { width: img.width(), height: img.height(), rgba: img.into_raw() })
}

// ───────────────────────────── Cadres ─────────────────────────────

/// Carte claire bordée (contrôles du lecteur, réglages…).
pub fn card(radius: u8, margin: Margin) -> Frame {
    Frame::new()
        .fill(PANEL)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(radius))
        .inner_margin(margin)
}

/// Intitulé de section en petites capitales espacées.
pub fn section_label(ui: &mut Ui, text: &str, color: Color32) {
    ui.label(t(text.to_uppercase(), 11.0, W::Black, color).extra_letter_spacing(1.3));
}

/// Pastille de texte bordée.
pub fn chip(ui: &mut Ui, text: &str, bg: Option<Color32>) {
    let mut f = Frame::new()
        .stroke(Stroke::new(1.0, BORDER_INPUT))
        .corner_radius(CornerRadius::same(255))
        .inner_margin(Margin::symmetric(11, 5));
    if let Some(bg) = bg {
        f = f.fill(bg);
    }
    f.show(ui, |ui| ui.label(t(text, 12.0, W::Semi, MUTED)));
}

// ───────────────────────────── Boutons ─────────────────────────────

#[allow(clippy::too_many_arguments)]
fn button_core(
    ui: &mut Ui,
    text: &str,
    font_id: FontId,
    color: Color32,
    size: Vec2,
    enabled: bool,
    fill: (Color32, Color32),
    stroke: (Color32, Color32),
    radius: u8,
) -> Response {
    let (rect, resp) = ui.allocate_exact_size(size, if enabled { Sense::click() } else { Sense::hover() });
    let hovered = enabled && resp.hovered();
    let painter = ui.painter();
    let fill = if hovered { fill.1 } else { fill.0 };
    let stroke_c = if hovered { stroke.1 } else { stroke.0 };
    let alpha = if enabled { 1.0 } else { 0.55 };
    painter.rect(
        rect,
        CornerRadius::same(radius),
        fill.gamma_multiply(alpha),
        Stroke::new(if stroke_c == Color32::TRANSPARENT { 0.0 } else { 1.0 }, stroke_c.gamma_multiply(alpha)),
        StrokeKind::Inside,
    );
    let galley = ui.painter().layout_no_wrap(text.to_string(), font_id, color);
    let color = if hovered && stroke.1 == ROSE { ROSE } else { color };
    ui.painter().galley_with_override_text_color(rect.center() - galley.size() / 2.0, galley, color.gamma_multiply(alpha));
    if enabled { resp.on_hover_cursor(CursorIcon::PointingHand) } else { resp }
}

/// Bouton principal noir prune.
pub fn primary(ui: &mut Ui, text: &str, size: Vec2, font_size: f32, enabled: bool) -> Response {
    let tr = Color32::TRANSPARENT;
    button_core(ui, text, font(font_size, W::Black), CREAM, size, enabled, (PLUM, PLUM_HOVER), (tr, tr), 14)
}

/// Bouton principal dont la largeur suit le texte.
pub fn primary_fit(ui: &mut Ui, text: &str, font_size: f32, height: f32) -> Response {
    let w = text_width(ui, text, &font(font_size, W::Black)) + 36.0;
    primary(ui, text, Vec2::new(w, height), font_size, true)
}

/// Bouton secondaire clair bordé (devient rose au survol).
pub fn secondary(ui: &mut Ui, text: &str) -> Response {
    let f = font(13.0, W::Bold);
    let w = text_width(ui, text, &f) + 26.0;
    button_core(ui, text, f, INK, Vec2::new(w, 34.0), true, (PANEL, PANEL), (BORDER_INPUT, ROSE), 10)
}

/// Bouton secondaire de taille imposée.
pub fn secondary_sized(ui: &mut Ui, text: &str, size: Vec2, radius: u8) -> Response {
    button_core(ui, text, font(13.0, W::Bold), INK, size, true, (PANEL, PANEL), (BORDER_INPUT, ROSE), radius)
}

/// Lien texte (rose ou discret).
pub fn link(ui: &mut Ui, text: &str, color: Color32) -> Response {
    let r = ui.add(egui::Label::new(t(text, 13.0, W::Bold, color)).sense(Sense::click()));
    let r = r.on_hover_cursor(CursorIcon::PointingHand);
    if r.hovered() {
        ui.painter().hline(r.rect.x_range(), r.rect.bottom(), Stroke::new(1.0, ROSE));
    }
    r
}

fn text_width(ui: &Ui, text: &str, f: &FontId) -> f32 {
    ui.painter().layout_no_wrap(text.to_string(), f.clone(), INK).size().x
}

// ───────────────────────────── Interrupteurs ─────────────────────────────

/// Interrupteur rose 38×22.
pub fn switch(ui: &mut Ui, on: &mut bool) -> Response {
    let (rect, mut resp) = ui.allocate_exact_size(Vec2::new(38.0, 22.0), Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    paint_switch(ui, rect, *on, resp.id);
    resp.on_hover_cursor(CursorIcon::PointingHand)
}

fn paint_switch(ui: &Ui, rect: Rect, on: bool, id: egui::Id) {
    let k = ui.ctx().animate_bool_with_time(id, on, 0.15);
    let bg = lerp_color(SWITCH_OFF, ROSE, k);
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(11), bg);
    let x = egui::lerp(rect.left() + 11.0..=rect.right() - 11.0, k);
    p.circle_filled(Pos2::new(x, rect.center().y + 0.5), 9.5, Color32::from_black_alpha(35));
    p.circle_filled(Pos2::new(x, rect.center().y), 9.0, Color32::WHITE);
}

/// Ligne « libellé … interrupteur » sur toute la largeur.
pub fn switch_row(ui: &mut Ui, label: &str, on: &mut bool) -> Response {
    let width = ui.available_width();
    let galley = ui.painter().layout(label.to_string(), font(14.0, W::Semi), INK, width - 52.0);
    let h = galley.size().y.max(22.0);
    let (rect, mut resp) = ui.allocate_exact_size(Vec2::new(width, h), Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    ui.painter().galley(Pos2::new(rect.left(), rect.center().y - galley.size().y / 2.0), galley, INK);
    let sw = Rect::from_min_size(Pos2::new(rect.right() - 38.0, rect.center().y - 11.0), Vec2::new(38.0, 22.0));
    paint_switch(ui, sw, *on, resp.id);
    resp.on_hover_cursor(CursorIcon::PointingHand)
}

/// Case à cocher arrondie.
pub fn checkbox(ui: &mut Ui, on: &mut bool, label: &str, enabled: bool) -> Response {
    let galley = ui.painter().layout_no_wrap(label.to_string(), font(14.0, W::Semi), INK);
    let size = Vec2::new(28.0 + galley.size().x, galley.size().y.max(20.0));
    let (rect, mut resp) = ui.allocate_exact_size(size, if enabled { Sense::click() } else { Sense::hover() });
    if enabled && resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let alpha = if enabled { 1.0 } else { 0.45 };
    let bx = Rect::from_min_size(Pos2::new(rect.left(), rect.center().y - 9.0), Vec2::splat(18.0));
    let p = ui.painter();
    if *on {
        p.rect_filled(bx, CornerRadius::same(6), ROSE.gamma_multiply(alpha));
        let c = bx.center();
        p.line(
            vec![c + Vec2::new(-4.5, 0.2), c + Vec2::new(-1.2, 3.4), c + Vec2::new(4.6, -3.4)],
            Stroke::new(2.0, Color32::WHITE),
        );
    } else {
        p.rect(bx, CornerRadius::same(6), Color32::WHITE, Stroke::new(1.5, CHECK_BORDER.gamma_multiply(alpha)), StrokeKind::Inside);
    }
    p.galley_with_override_text_color(Pos2::new(bx.right() + 10.0, rect.center().y - galley.size().y / 2.0), galley, INK.gamma_multiply(alpha));
    if enabled { resp.on_hover_cursor(CursorIcon::PointingHand) } else { resp }
}

/// Carte de choix (méthode de suppression de la voix).
pub fn choice_card(ui: &mut Ui, selected: bool, title: &str, subtitle: &str, width: f32) -> Response {
    // Le sous-titre passe à la ligne si la carte est étroite.
    let sub = ui.painter().layout(subtitle.to_string(), font(12.0, W::Medium), MUTED, width - 24.0);
    let height = 38.0 + sub.size().y + 13.0;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, height), Sense::click());
    let p = ui.painter();
    let (bg, bd) = if selected { (ROSE_CARD, ROSE) } else { (Color32::WHITE, if resp.hovered() { ROSE_LIGHT } else { BORDER }) };
    p.rect(rect, CornerRadius::same(14), bg, Stroke::new(1.5, bd), StrokeKind::Inside);
    let c = Pos2::new(rect.left() + 19.0, rect.top() + 22.0);
    p.circle_stroke(c, 7.0, Stroke::new(1.5, if selected { ROSE } else { CHECK_BORDER }));
    if selected {
        p.circle_filled(c, 3.0, ROSE);
    }
    p.text(Pos2::new(rect.left() + 34.0, c.y), Align2::LEFT_CENTER, title, font(14.0, W::Black), INK);
    p.galley(Pos2::new(rect.left() + 12.0, rect.top() + 36.0), sub, MUTED);
    resp.on_hover_cursor(CursorIcon::PointingHand)
}

// ───────────────────────────── Curseurs ─────────────────────────────

/// Curseur à pastille : piste rose remplie depuis `origin`, bouton blanc cerclé de rose.
/// Renvoie la réponse (`changed`, `dragged`, `drag_stopped`).
pub fn slider(ui: &mut Ui, value: &mut f64, range: std::ops::RangeInclusive<f64>, step: f64, origin: f64, width: f32) -> Response {
    let (rect, mut resp) = ui.allocate_exact_size(Vec2::new(width, 20.0), Sense::click_and_drag());
    let (lo, hi) = (*range.start(), *range.end());
    let x0 = rect.left() + 8.0;
    let x1 = rect.right() - 8.0;
    if let Some(pos) = resp.interact_pointer_pos() {
        if resp.dragged() || resp.clicked() || resp.drag_started() {
            let f = ((pos.x - x0) / (x1 - x0)).clamp(0.0, 1.0) as f64;
            let mut v = lo + f * (hi - lo);
            if step > 0.0 {
                v = (v / step).round() * step;
            }
            let v = v.clamp(lo, hi);
            if v != *value {
                *value = v;
                resp.mark_changed();
            }
        }
    }
    let to_x = |v: f64| x0 + ((v - lo) / (hi - lo)).clamp(0.0, 1.0) as f32 * (x1 - x0);
    let p = ui.painter();
    let cy = rect.center().y;
    let track = Rect::from_min_max(Pos2::new(rect.left(), cy - 3.0), Pos2::new(rect.right(), cy + 3.0));
    p.rect_filled(track, CornerRadius::same(3), TRACK);
    let (a, b) = (to_x(origin.clamp(lo, hi)), to_x(*value));
    let fill = Rect::from_min_max(Pos2::new(a.min(b), cy - 3.0), Pos2::new(a.max(b), cy + 3.0));
    p.rect_filled(fill, CornerRadius::same(3), ROSE);
    let knob = Pos2::new(b, cy);
    let r = if resp.hovered() || resp.dragged() { 9.0 } else { 8.0 };
    p.circle_filled(knob + Vec2::new(0.0, 1.5), r, ROSE.gamma_multiply(0.25));
    p.circle(knob, r, Color32::WHITE, Stroke::new(2.0, ROSE));
    resp.on_hover_cursor(CursorIcon::PointingHand)
}

/// Sélecteur à deux segments (« Voix retirée » / « Originale »).
pub fn segmented(ui: &mut Ui, right_selected: &mut bool, left: &str, right: &str, enabled: bool) -> Response {
    let fl = font(13.0, W::Bold);
    let gl = ui.painter().layout_no_wrap(left.to_string(), fl.clone(), INK);
    let gr = ui.painter().layout_no_wrap(right.to_string(), fl, INK);
    let (wl, wr) = (gl.size().x + 24.0, gr.size().x + 24.0);
    let (rect, mut resp) = ui.allocate_exact_size(Vec2::new(wl + wr + 8.0, 40.0), if enabled { Sense::click() } else { Sense::hover() });
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(12), SEGMENT_BG);
    let lr = Rect::from_min_size(rect.min + Vec2::new(4.0, 4.0), Vec2::new(wl, 32.0));
    let rr = Rect::from_min_size(Pos2::new(lr.right(), lr.top()), Vec2::new(wr, 32.0));
    if enabled && resp.clicked() {
        if let Some(pos) = resp.interact_pointer_pos() {
            let new = rr.contains(pos) || pos.x > rr.left();
            if new != *right_selected {
                *right_selected = new;
                resp.mark_changed();
            }
        }
    }
    let alpha = if enabled { 1.0 } else { 0.5 };
    for (r, g, sel) in [(lr, gl, !*right_selected), (rr, gr, *right_selected)] {
        if sel {
            p.rect_filled(r, CornerRadius::same(9), PLUM.gamma_multiply(alpha));
        }
        let c = if sel { CREAM } else { MUTED };
        p.galley_with_override_text_color(r.center() - g.size() / 2.0, g, c.gamma_multiply(alpha));
    }
    if enabled { resp.on_hover_cursor(CursorIcon::PointingHand) } else { resp }
}

/// Touche de clavier (« V »).
pub fn keycap(ui: &mut Ui, key: &str) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(22.0, 22.0), Sense::hover());
    let p = ui.painter();
    p.rect_stroke(rect, CornerRadius::same(6), Stroke::new(1.0, rgb(0xddd0d6)), StrokeKind::Inside);
    p.text(rect.center(), Align2::CENTER_CENTER, key, egui::FontId::monospace(11.0), FAINT);
}

/// Pas à pas « − valeur + ». Renvoie vrai si la valeur a changé.
pub fn stepper(ui: &mut Ui, value: &mut f64, step: f64, range: std::ops::RangeInclusive<f64>, label: &str) -> bool {
    let mut changed = false;
    Frame::new()
        .fill(PANEL)
        .stroke(Stroke::new(1.0, BORDER_INPUT))
        .corner_radius(CornerRadius::same(10))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.horizontal(|ui| {
                let btn = |ui: &mut Ui, s: &str| {
                    let (r, resp) = ui.allocate_exact_size(Vec2::new(30.0, 32.0), Sense::click());
                    let c = if resp.hovered() { ROSE } else { INK };
                    ui.painter().text(r.center(), Align2::CENTER_CENTER, s, font(16.0, W::Bold), c);
                    resp.on_hover_cursor(CursorIcon::PointingHand).clicked()
                };
                if btn(ui, "−") {
                    *value = (*value - step).max(*range.start());
                    changed = true;
                }
                let (r, _) = ui.allocate_exact_size(Vec2::new(62.0, 32.0), Sense::hover());
                ui.painter().text(r.center(), Align2::CENTER_CENTER, label, font(13.0, W::Black), INK);
                if btn(ui, "+") {
                    *value = (*value + step).min(*range.end());
                    changed = true;
                }
            });
        });
    if changed {
        *value = (*value * 100.0).round() / 100.0;
    }
    changed
}

/// Bouton rond Lecture / Pause.
pub fn play_button(ui: &mut Ui, playing: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(48.0), Sense::click());
    let p = ui.painter();
    let c = rect.center();
    p.circle_filled(c, 24.0, if resp.hovered() { PLUM_HOVER } else { PLUM });
    if playing {
        for dx in [-4.0, 4.0] {
            p.rect_filled(Rect::from_center_size(c + Vec2::new(dx, 0.0), Vec2::new(4.0, 15.0)), CornerRadius::same(1), CREAM);
        }
    } else {
        let pts = vec![c + Vec2::new(-5.0, -8.0), c + Vec2::new(-5.0, 8.0), c + Vec2::new(8.0, 0.0)];
        p.add(Shape::convex_polygon(pts, CREAM, Stroke::NONE));
    }
    resp.on_hover_cursor(CursorIcon::PointingHand)
}

/// Barre de progression arrondie en dégradé rose.
pub fn progress(ui: &mut Ui, frac: f32, width: f32, height: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    let r = CornerRadius::same((height / 2.0) as u8);
    ui.painter().rect_filled(rect, r, TRACK);
    let w = rect.width() * frac.clamp(0.0, 1.0);
    if w > 1.0 {
        let fill = Rect::from_min_size(rect.min, Vec2::new(w.max(height), height));
        // dégradé horizontal approché par deux tons
        ui.painter().rect_filled(fill, r, ROSE);
        let left = Rect::from_min_size(rect.min, Vec2::new(fill.width() * 0.5, height));
        ui.painter().rect_filled(left, r, ROSE_LIGHT.gamma_multiply(0.6));
    }
}

// ───────────────────────────── Dessins ─────────────────────────────

/// Dégradé vertical (haut → bas).
pub fn vertical_gradient(painter: &egui::Painter, rect: Rect, top: Color32, bottom: Color32) {
    let mut mesh = Mesh::default();
    mesh.colored_vertex(rect.left_top(), top);
    mesh.colored_vertex(rect.right_top(), top);
    mesh.colored_vertex(rect.left_bottom(), bottom);
    mesh.colored_vertex(rect.right_bottom(), bottom);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(1, 2, 3);
    painter.add(Shape::mesh(mesh));
}

/// Disque en dégradé radial (centre → bord).
pub fn radial_disc(painter: &egui::Painter, center: Pos2, radius: f32, inner: Color32, outer: Color32) {
    let mut mesh = Mesh::default();
    mesh.colored_vertex(center, inner);
    let n = 64;
    for i in 0..=n {
        let a = i as f32 / n as f32 * std::f32::consts::TAU;
        mesh.colored_vertex(center + Vec2::angled(a) * radius, outer);
    }
    for i in 1..=n as u32 {
        mesh.add_triangle(0, i, i + 1);
    }
    painter.add(Shape::mesh(mesh));
}

/// Logo dans un disque coloré.
pub fn logo_disc(ui: &mut Ui, logo: &TextureHandle, diameter: f32, logo_size: f32, bg: Color32, dy: f32) -> Rect {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(diameter), Sense::hover());
    ui.painter().circle_filled(rect.center(), diameter / 2.0, bg);
    paint_logo(ui.painter(), logo, rect.center() + Vec2::new(0.0, dy), logo_size);
    rect
}

/// Dessine le logo centré en `center`, contenu dans un carré `size`.
pub fn paint_logo(painter: &egui::Painter, logo: &TextureHandle, center: Pos2, size: f32) {
    let [w, h] = logo.size();
    let s = size / w.max(h) as f32;
    let r = Rect::from_center_size(center, Vec2::new(w as f32 * s, h as f32 * s));
    painter.image(logo.id(), r, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
}

pub fn lerp_color(a: Color32, b: Color32, k: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * k).round() as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}
