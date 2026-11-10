use std::sync::Arc;

use eframe::egui::{
    self, Color32, CornerRadius, CursorIcon, FontData, FontDefinitions, FontFamily, FontId, Margin,
    Response, RichText, Sense, Stroke, Ui, Vec2,
};

use crate::service::NoticeLevel;

pub const BOLD_FAMILY: &str = "peerprint-bold";
const ICON_FAMILY: &str = "peerprint-icons";
static ICON_FONT_READY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

#[derive(Clone, Copy)]
pub struct Palette {
    pub dark: bool,
    pub bg: Color32,
    pub sidebar: Color32,
    pub card: Color32,
    pub hover: Color32,
    pub border: Color32,
    pub border_strong: Color32,
    pub text: Color32,
    pub secondary: Color32,
    pub muted: Color32,
    pub faint: Color32,
    pub input: Color32,
    pub accent: Color32,
    pub accent_text: Color32,
    pub on_accent: Color32,
    pub accent_soft: Color32,
    pub track: Color32,
    pub success: Color32,
    pub warn: Color32,
    pub danger: Color32,
    pub info: Color32,
}

impl Palette {
    pub fn of(dark: bool) -> Self {
        if dark {
            Self {
                dark,
                bg: Color32::from_rgb(13, 17, 23),
                sidebar: Color32::from_rgb(17, 22, 29),
                card: Color32::from_rgb(22, 28, 36),
                hover: Color32::from_rgb(29, 36, 46),
                border: Color32::from_rgb(36, 45, 56),
                border_strong: Color32::from_rgb(52, 63, 77),
                text: Color32::from_rgb(230, 234, 240),
                secondary: Color32::from_rgb(156, 166, 180),
                muted: Color32::from_rgb(116, 126, 140),
                faint: Color32::from_rgb(26, 33, 42),
                input: Color32::from_rgb(15, 20, 26),
                accent: Color32::from_rgb(146, 193, 62),
                accent_text: Color32::from_rgb(146, 193, 62),
                on_accent: Color32::from_rgb(20, 30, 9),
                accent_soft: Color32::from_rgba_unmultiplied(146, 193, 62, 30),
                track: Color32::from_rgb(45, 55, 68),
                success: Color32::from_rgb(74, 222, 128),
                warn: Color32::from_rgb(251, 191, 36),
                danger: Color32::from_rgb(248, 113, 113),
                info: Color32::from_rgb(96, 165, 250),
            }
        } else {
            Self {
                dark,
                bg: Color32::from_rgb(245, 246, 248),
                sidebar: Color32::from_rgb(252, 252, 253),
                card: Color32::WHITE,
                hover: Color32::from_rgb(243, 245, 247),
                border: Color32::from_rgb(229, 232, 236),
                border_strong: Color32::from_rgb(211, 216, 223),
                text: Color32::from_rgb(24, 28, 34),
                secondary: Color32::from_rgb(75, 85, 99),
                muted: Color32::from_rgb(112, 122, 136),
                faint: Color32::from_rgb(243, 245, 248),
                input: Color32::from_rgb(246, 247, 249),
                accent: Color32::from_rgb(146, 193, 62),
                accent_text: Color32::from_rgb(78, 116, 21),
                on_accent: Color32::from_rgb(26, 36, 11),
                accent_soft: Color32::from_rgb(237, 246, 226),
                track: Color32::from_rgb(209, 215, 222),
                success: Color32::from_rgb(5, 122, 72),
                warn: Color32::from_rgb(154, 91, 0),
                danger: Color32::from_rgb(180, 35, 42),
                info: Color32::from_rgb(29, 78, 216),
            }
        }
    }
}

pub fn palette(ui: &Ui) -> Palette {
    Palette::of(ui.visuals().dark_mode)
}

pub fn bold() -> FontFamily {
    FontFamily::Name(BOLD_FAMILY.into())
}

/// The Windows icon font, when it could be loaded. Callers must keep a painter fallback.
pub fn icon_font() -> Option<FontFamily> {
    ICON_FONT_READY
        .get()
        .copied()
        .unwrap_or(false)
        .then(|| FontFamily::Name(ICON_FAMILY.into()))
}

pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let regular = ["SegUIVar.ttf", "segoeui.ttf"]
        .iter()
        .find_map(|name| std::fs::read(format!("C:\\Windows\\Fonts\\{name}")).ok());
    let mut proportional_fallback = fonts
        .families
        .get(&FontFamily::Proportional)
        .and_then(|family| family.first().cloned());
    if let Some(bytes) = regular {
        fonts
            .font_data
            .insert("peerprint-ui".to_owned(), Arc::new(FontData::from_owned(bytes)));
        fonts
            .families
            .get_mut(&FontFamily::Proportional)
            .expect("proportional family")
            .insert(0, "peerprint-ui".to_owned());
        proportional_fallback = Some("peerprint-ui".to_owned());
    }
    if let Ok(bytes) = std::fs::read("C:\\Windows\\Fonts\\seguisb.ttf") {
        fonts.font_data.insert(
            "peerprint-ui-bold".to_owned(),
            Arc::new(FontData::from_owned(bytes)),
        );
        let mut family = vec!["peerprint-ui-bold".to_owned()];
        if let Some(fallback) = proportional_fallback {
            family.push(fallback);
        }
        fonts.families.insert(bold(), family);
    }
    let icons = ["SegoeIcons.ttf", "segmdl2.ttf"]
        .iter()
        .find_map(|name| std::fs::read(format!("C:\\Windows\\Fonts\\{name}")).ok());
    if let Some(bytes) = icons {
        fonts.font_data.insert(
            ICON_FAMILY.to_owned(),
            Arc::new(FontData::from_owned(bytes)),
        );
        fonts.families.insert(
            FontFamily::Name(ICON_FAMILY.into()),
            vec![ICON_FAMILY.to_owned()],
        );
        let _ = ICON_FONT_READY.set(true);
    }
    ctx.set_fonts(fonts);
    ctx.style_mut_of(egui::Theme::Light, |style| apply_style(style, false));
    ctx.style_mut_of(egui::Theme::Dark, |style| apply_style(style, true));
}

fn apply_style(style: &mut egui::Style, dark: bool) {
    let p = Palette::of(dark);

    style.text_styles = [
        (egui::TextStyle::Heading, FontId::new(22.0, bold())),
        (
            egui::TextStyle::Body,
            FontId::new(13.5, FontFamily::Proportional),
        ),
        (
            egui::TextStyle::Button,
            FontId::new(13.0, FontFamily::Proportional),
        ),
        (
            egui::TextStyle::Small,
            FontId::new(11.5, FontFamily::Proportional),
        ),
        (
            egui::TextStyle::Monospace,
            FontId::new(12.0, FontFamily::Monospace),
        ),
    ]
    .into();
    style.spacing.item_spacing = Vec2::new(8.0, 8.0);
    style.spacing.button_padding = Vec2::new(11.0, 6.0);
    style.spacing.interact_size = Vec2::new(32.0, 26.0);
    style.spacing.window_margin = Margin::same(20);
    style.spacing.menu_margin = Margin::same(10);
    style.spacing.indent = 18.0;
    style.spacing.scroll.bar_width = 8.0;
    style.spacing.scroll.floating = true;
    style.spacing.scroll.floating_allocated_width = 4.0;

    let mut v = if dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    v.dark_mode = dark;
    v.override_text_color = Some(p.text);
    v.panel_fill = p.bg;
    v.window_fill = p.card;
    v.window_stroke = Stroke::new(1.0, p.border);
    v.window_corner_radius = CornerRadius::same(16);
    v.menu_corner_radius = CornerRadius::same(12);
    v.window_shadow = egui::epaint::Shadow {
        offset: [0, 10],
        blur: 28,
        spread: 0,
        color: Color32::from_black_alpha(if dark { 90 } else { 36 }),
    };
    v.popup_shadow = egui::epaint::Shadow {
        offset: [0, 6],
        blur: 18,
        spread: 0,
        color: Color32::from_black_alpha(if dark { 80 } else { 28 }),
    };
    v.faint_bg_color = p.faint;
    v.extreme_bg_color = p.input;
    v.selection.bg_fill = Color32::from_rgba_unmultiplied(p.accent.r(), p.accent.g(), p.accent.b(), 64);
    v.selection.stroke = Stroke::new(1.0, p.accent_text);
    v.hyperlink_color = p.accent_text;
    v.text_cursor.stroke = Stroke::new(1.5, p.accent_text);

    v.widgets.noninteractive.bg_fill = p.card;
    v.widgets.noninteractive.weak_bg_fill = p.card;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, p.secondary);
    v.widgets.noninteractive.corner_radius = CornerRadius::same(10);

    v.widgets.inactive.bg_fill = p.input;
    v.widgets.inactive.weak_bg_fill = p.card;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, p.border);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, p.text);
    v.widgets.inactive.corner_radius = CornerRadius::same(9);
    v.widgets.inactive.expansion = 0.0;

    v.widgets.hovered.bg_fill = p.faint;
    v.widgets.hovered.weak_bg_fill = p.hover;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, p.border_strong);
    v.widgets.hovered.fg_stroke = Stroke::new(1.0, p.text);
    v.widgets.hovered.corner_radius = CornerRadius::same(9);
    v.widgets.hovered.expansion = 0.0;

    v.widgets.active.bg_fill = p.faint;
    v.widgets.active.weak_bg_fill = p.hover;
    v.widgets.active.bg_stroke = Stroke::new(1.0, p.border_strong);
    v.widgets.active.fg_stroke = Stroke::new(1.0, p.text);
    v.widgets.active.corner_radius = CornerRadius::same(9);
    v.widgets.active.expansion = 0.0;

    v.widgets.open = v.widgets.active;

    style.visuals = v;
}

pub fn page_header(ui: &mut Ui, title: &str, subtitle: &str) {
    let p = palette(ui);
    ui.add_space(2.0);
    ui.label(
        RichText::new(title)
            .font(FontId::new(24.0, bold()))
            .color(p.text),
    );
    ui.add_space(1.0);
    ui.label(RichText::new(subtitle).size(13.0).color(p.secondary));
    ui.add_space(16.0);
}

pub fn section_title(ui: &mut Ui, text: &str) {
    let p = palette(ui);
    ui.label(
        RichText::new(text)
            .font(FontId::new(15.0, bold()))
            .color(p.text),
    );
}

pub fn card<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let p = palette(ui);
    egui::Frame::new()
        .fill(p.card)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(14)
        .inner_margin(Margin::same(18))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

pub fn list_card<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let p = palette(ui);
    egui::Frame::new()
        .fill(p.card)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(14)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

pub fn list_row<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::new()
        .inner_margin(Margin {
            left: 18,
            right: 18,
            top: 13,
            bottom: 13,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

pub fn divider(ui: &mut Ui) {
    let p = palette(ui);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
    ui.painter()
        .hline(rect.x_range(), rect.center().y, Stroke::new(1.0, p.border));
}

pub fn dot(ui: &mut Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(7.0), Sense::hover());
    ui.painter().circle_filled(rect.center(), 3.0, color);
}

pub fn pill(ui: &mut Ui, text: &str, color: Color32) -> Response {
    let bg = Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 28);
    let border = Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 70);
    egui::Frame::new()
        .fill(bg)
        .stroke(Stroke::new(1.0, border))
        .corner_radius(CornerRadius::same(99))
        .inner_margin(Margin::symmetric(9, 3))
        .show(ui, |ui| {
            ui.label(RichText::new(text).size(11.0).color(color));
        })
        .response
}

pub fn primary_button(ui: &mut Ui, text: &str) -> Response {
    let p = palette(ui);
    ui.add(
        egui::Button::new(RichText::new(text).size(13.0).color(p.on_accent))
            .fill(p.accent)
            .stroke(Stroke::NONE)
            .corner_radius(9)
            .min_size(Vec2::new(0.0, 30.0)),
    )
    .on_hover_cursor(CursorIcon::PointingHand)
}

pub fn secondary_button(ui: &mut Ui, text: &str) -> Response {
    let p = palette(ui);
    ui.add(
        egui::Button::new(RichText::new(text).size(12.0).color(p.text))
            .fill(p.card)
            .stroke(Stroke::new(1.0, p.border))
            .corner_radius(8)
            .min_size(Vec2::new(0.0, 28.0)),
    )
    .on_hover_cursor(CursorIcon::PointingHand)
}

pub fn danger_button(ui: &mut Ui, text: &str) -> Response {
    let p = palette(ui);
    ui.add(
        egui::Button::new(RichText::new(text).size(12.0).color(p.danger))
            .fill(p.card)
            .stroke(Stroke::new(1.0, p.border))
            .corner_radius(8)
            .min_size(Vec2::new(0.0, 28.0)),
    )
    .on_hover_cursor(CursorIcon::PointingHand)
}

pub fn tiny_button(ui: &mut Ui, text: &str) -> Response {
    let p = palette(ui);
    ui.add(
        egui::Button::new(RichText::new(text).size(11.5).color(p.secondary))
            .fill(p.card)
            .stroke(Stroke::new(1.0, p.border))
            .corner_radius(7)
            .min_size(Vec2::new(58.0, 24.0)),
    )
    .on_hover_cursor(CursorIcon::PointingHand)
}

pub fn toggle(ui: &mut Ui, on: &mut bool) -> Response {
    let p = palette(ui);
    let desired = Vec2::new(38.0, 22.0);
    let (rect, mut response) = ui.allocate_exact_size(desired, Sense::click());
    response = response.on_hover_cursor(CursorIcon::PointingHand);
    if ui.is_enabled() && response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    let t = ui
        .ctx()
        .animate_bool_with_time(response.id, *on && ui.is_enabled(), 0.12);
    let track = if ui.is_enabled() {
        p.track.lerp_to_gamma(p.accent, t)
    } else {
        p.track
    };
    ui.painter().rect_filled(rect, CornerRadius::same(11), track);
    let knob_x = egui::lerp((rect.left() + 11.0)..=(rect.right() - 11.0), t);
    let knob = if ui.is_enabled() {
        Color32::WHITE
    } else {
        p.muted
    };
    ui.painter()
        .circle_filled(egui::pos2(knob_x, rect.center().y), 8.0, knob);
    response
}

pub fn empty_card(ui: &mut Ui, title: &str, hint: &str) {
    let p = palette(ui);
    card(ui, |ui| {
        ui.add_space(10.0);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new(title).size(13.0).color(p.secondary));
            ui.label(RichText::new(hint).size(11.5).color(p.muted));
        });
        ui.add_space(10.0);
    });
}

pub fn notice(ui: &mut Ui, level: NoticeLevel, text: &str) {
    let p = palette(ui);
    let color = match level {
        NoticeLevel::Info => p.info,
        NoticeLevel::Warn => p.warn,
        NoticeLevel::Error => p.danger,
    };
    let bg = Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), if p.dark { 34 } else { 22 });
    let border = Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), if p.dark { 90 } else { 60 });
    egui::Frame::new()
        .fill(bg)
        .stroke(Stroke::new(1.0, border))
        .corner_radius(10)
        .inner_margin(Margin::symmetric(12, 9))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                dot(ui, color);
                ui.add(
                    egui::Label::new(RichText::new(text).size(12.0).color(p.text)).wrap(),
                );
            });
        });
}

pub fn inline_warning(ui: &mut Ui, text: &str, action: impl FnOnce(&mut Ui)) {
    let p = palette(ui);
    let bg = Color32::from_rgba_unmultiplied(p.warn.r(), p.warn.g(), p.warn.b(), if p.dark { 34 } else { 20 });
    let border = Color32::from_rgba_unmultiplied(p.warn.r(), p.warn.g(), p.warn.b(), if p.dark { 90 } else { 55 });
    egui::Frame::new()
        .fill(bg)
        .stroke(Stroke::new(1.0, border))
        .corner_radius(10)
        .inner_margin(Margin::symmetric(12, 9))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                dot(ui, p.warn);
                ui.add(
                    egui::Label::new(RichText::new(text).size(12.0).color(p.text)).wrap(),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), action);
            });
        });
}

pub fn text_input(ui: &mut Ui, text: &mut String, width: f32, hint: &str) -> Response {
    ui.add(
        egui::TextEdit::singleline(text)
            .desired_width(width)
            .hint_text(hint)
            .margin(Margin::symmetric(10, 6)),
    )
}

pub fn password_input(ui: &mut Ui, text: &mut String, width: f32, hint: &str) -> Response {
    ui.add(
        egui::TextEdit::singleline(text)
            .password(true)
            .desired_width(width)
            .hint_text(hint)
            .margin(Margin::symmetric(10, 6)),
    )
}

pub fn label(ui: &mut Ui, text: &str) {
    let p = palette(ui);
    ui.label(RichText::new(text).size(11.5).color(p.muted));
}

pub fn muted(ui: &mut Ui, text: &str) {
    let p = palette(ui);
    ui.label(RichText::new(text).size(11.5).color(p.muted));
}

pub fn dialog_frame(ui: &Ui) -> egui::Frame {
    let p = palette(ui);
    egui::Frame::new()
        .fill(p.card)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(18)
        .inner_margin(Margin::same(24))
        .shadow(egui::epaint::Shadow {
            offset: [0, 16],
            blur: 48,
            spread: 0,
            color: Color32::from_black_alpha(if p.dark { 120 } else { 48 }),
        })
}

pub fn state_color(p: &Palette, state: &str) -> Color32 {
    match state {
        "Printed" => p.success,
        "Failed" | "Revoked" => p.danger,
        "Waiting for host" => p.warn,
        "Cancelled" => p.muted,
        "Queued" | "Uploading" | "Receiving" | "Spooling" | "Printing" => p.info,
        _ => p.muted,
    }
}
