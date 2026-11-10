use std::time::Duration;

use eframe::egui::{
    self, Align2, Color32, CornerRadius, CursorIcon, FontId, Id, Layout, Margin, Pos2, Rect,
    RichText, Sense, Stroke, Vec2,
};

use crate::service::{CoreState, NoticeLevel};
use crate::ui::{format_age, theme, theme::Palette, App};

const FLYOUT_SECONDS: f64 = 5.0;
const FLYOUT_FADE: f64 = 0.35;
const FLYOUT_WIDTH: f32 = 340.0;
const MAX_FLYOUTS: usize = 4;

#[derive(Clone)]
pub struct Flyout {
    pub id: u64,
    pub level: NoticeLevel,
    pub message: String,
    pub shown_at: f64,
}

pub fn level_color(p: &Palette, level: NoticeLevel) -> Color32 {
    match level {
        NoticeLevel::Info => p.info,
        NoticeLevel::Warn => p.warn,
        NoticeLevel::Error => p.danger,
    }
}

/// Turns notices that arrived since the last frame into short-lived flyouts.
pub fn sync(app: &mut App, ctx: &egui::Context, core: &CoreState) {
    let now = ctx.input(|input| input.time);
    let newest = core.notices.front().map(|notice| notice.id).unwrap_or(0);
    if !app.state.notices_ready {
        if !app.backend.connected() {
            return;
        }
        app.state.seen_notice = app.state.seen_notice.max(newest);
        app.state.flyout_notice = app.state.flyout_notice.max(newest);
        app.state.notices_ready = true;
        return;
    }

    let fresh_notices: Vec<_> = core
        .notices
        .iter()
        .filter(|notice| notice.id > app.state.flyout_notice)
        .collect();
    if !fresh_notices.is_empty() {
        let newest_fresh = fresh_notices
            .iter()
            .map(|notice| notice.id)
            .max()
            .unwrap_or(app.state.flyout_notice);
        app.state.flyout_notice = app.state.flyout_notice.max(newest_fresh);
        if app.state.notices_open {
            // Already visible in the open notification center.
            app.state.seen_notice = app.state.seen_notice.max(newest_fresh);
        } else {
            let mut fresh: Vec<Flyout> = fresh_notices
                .into_iter()
                .map(|notice| Flyout {
                    id: notice.id,
                    level: notice.level,
                    message: notice.message.clone(),
                    shown_at: now,
                })
                .collect();
            fresh.append(&mut app.state.flyouts);
            fresh.truncate(MAX_FLYOUTS);
            app.state.flyouts = fresh;
        }
    }

    if !app.state.flyouts.is_empty() {
        app.state
            .flyouts
            .retain(|flyout| now - flyout.shown_at < FLYOUT_SECONDS);
        if !app.state.flyouts.is_empty() {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
    }
}

pub fn unread(core: &CoreState, seen: u64) -> usize {
    core.notices
        .iter()
        .filter(|notice| notice.id > seen)
        .count()
}

/// The bell row that lives in the bottom-left of the sidebar.
pub fn button(ui: &mut egui::Ui, unread: usize, open: bool) -> egui::Response {
    let p = theme::palette(ui);
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 38.0), Sense::click());
    let response = response.on_hover_cursor(CursorIcon::PointingHand);
    let hover = ui.ctx().animate_bool(response.id, response.hovered());
    if open {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(19), p.accent_soft);
    } else if hover > 0.0 {
        ui.painter().rect_filled(
            rect,
            CornerRadius::same(19),
            Color32::TRANSPARENT.lerp_to_gamma(p.hover, hover),
        );
    }

    let color = if open { p.accent_text } else { p.secondary };
    paint_bell(ui.painter(), Pos2::new(rect.left() + 13.0, rect.center().y - 1.0), color);
    ui.painter().text(
        Pos2::new(rect.left() + 34.0, rect.center().y),
        Align2::LEFT_CENTER,
        "Notifications",
        FontId::new(
            13.0,
            if open {
                theme::bold()
            } else {
                egui::FontFamily::Proportional
            },
        ),
        color,
    );

    if unread > 0 {
        let badge = if unread > 9 {
            "9+".to_string()
        } else {
            unread.to_string()
        };
        let center = Pos2::new(rect.right() - 15.0, rect.center().y);
        ui.painter().circle_filled(center, 8.0, p.accent);
        ui.painter().text(
            center,
            Align2::CENTER_CENTER,
            badge,
            FontId::new(9.5, theme::bold()),
            p.on_accent,
        );
    }
    response
}

fn paint_bell(painter: &egui::Painter, center: Pos2, color: Color32) {
    if let Some(family) = theme::icon_font() {
        painter.text(
            center,
            Align2::CENTER_CENTER,
            "\u{EA8F}",
            FontId::new(15.0, family),
            color,
        );
        return;
    }
    let stroke = Stroke::new(1.5, color);
    let base_y = center.y + 3.0;
    let mut dome = Vec::with_capacity(18);
    for step in 0..=16 {
        let angle = std::f32::consts::PI + std::f32::consts::PI * step as f32 / 16.0;
        dome.push(Pos2::new(
            center.x + 7.0 * angle.cos(),
            base_y + 8.0 * angle.sin(),
        ));
    }
    painter.add(egui::Shape::line(dome, stroke));
    painter.line_segment(
        [
            Pos2::new(center.x - 9.0, base_y + 1.0),
            Pos2::new(center.x + 9.0, base_y + 1.0),
        ],
        stroke,
    );
    painter.circle_filled(Pos2::new(center.x, base_y + 4.5), 1.6, color);
    painter.line_segment(
        [
            Pos2::new(center.x, center.y - 5.0),
            Pos2::new(center.x, center.y - 7.0),
        ],
        stroke,
    );
}

fn overlay_offset(app: &App, ctx: &egui::Context) -> Option<Vec2> {
    let bell = app.state.bell_rect;
    if !bell.is_positive() {
        return None;
    }
    let screen = ctx.content_rect();
    Some(Vec2::new(
        bell.left() - screen.left(),
        bell.top() - 10.0 - screen.bottom(),
    ))
}

pub fn draw_flyouts(app: &mut App, ctx: &egui::Context) {
    if app.state.flyouts.is_empty() {
        return;
    }
    let Some(offset) = overlay_offset(app, ctx) else {
        return;
    };
    let now = ctx.input(|input| input.time);
    let items = app.state.flyouts.clone();
    let mut dismiss = None;
    egui::Area::new(Id::unique("notification_flyouts"))
        .order(egui::Order::Foreground)
        .anchor(Align2::LEFT_BOTTOM, offset)
        .fade_in(false)
        .layout(Layout::bottom_up(egui::Align::Min))
        .show(ctx, |ui| {
            ui.set_width(FLYOUT_WIDTH);
            ui.spacing_mut().item_spacing.y = 8.0;
            for flyout in &items {
                if draw_flyout(ui, flyout, now) {
                    dismiss = Some(flyout.id);
                }
            }
        });
    if let Some(id) = dismiss {
        app.state.flyouts.retain(|flyout| flyout.id != id);
    }
}

fn draw_flyout(ui: &mut egui::Ui, flyout: &Flyout, now: f64) -> bool {
    let p = theme::palette(ui);
    let age = (now - flyout.shown_at).max(0.0);
    let alpha = ((FLYOUT_SECONDS - age) / FLYOUT_FADE).clamp(0.0, 1.0) as f32;
    let color = level_color(&p, flyout.level);

    ui.scope(|ui| {
        ui.set_opacity(alpha);
        let frame = egui::Frame::new()
            .fill(p.card)
            .stroke(Stroke::new(
                1.0,
                Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 110),
            ))
            .corner_radius(CornerRadius::same(12))
            .inner_margin(Margin {
                left: 14,
                right: 30,
                top: 11,
                bottom: 11,
            })
            .shadow(egui::epaint::Shadow {
                offset: [0, 8],
                blur: 24,
                spread: 0,
                color: Color32::from_black_alpha(if p.dark { 90 } else { 30 }),
            })
            .show(ui, |ui| {
                ui.set_width(FLYOUT_WIDTH - 44.0);
                ui.horizontal(|ui| {
                    theme::dot(ui, color);
                    ui.add(
                        egui::Label::new(RichText::new(&flyout.message).size(12.0).color(p.text))
                            .wrap(),
                    );
                });
            });

        let rect = frame.response.rect;
        ui.painter().rect_filled(
            Rect::from_min_size(rect.min, Vec2::new(3.0, rect.height())),
            CornerRadius::same(2),
            color,
        );

        let close_center = Pos2::new(rect.right() - 15.0, rect.top() + 15.0);
        let close_rect = Rect::from_center_size(close_center, Vec2::splat(20.0));
        let close = ui
            .interact(
                close_rect,
                Id::unique(("flyout_close", flyout.id)),
                Sense::click(),
            )
            .on_hover_cursor(CursorIcon::PointingHand);
        let x_color = if close.hovered() { p.text } else { p.muted };
        let arm = 3.0;
        ui.painter().line_segment(
            [
                Pos2::new(close_center.x - arm, close_center.y - arm),
                Pos2::new(close_center.x + arm, close_center.y + arm),
            ],
            Stroke::new(1.4, x_color),
        );
        ui.painter().line_segment(
            [
                Pos2::new(close_center.x + arm, close_center.y - arm),
                Pos2::new(close_center.x - arm, close_center.y + arm),
            ],
            Stroke::new(1.4, x_color),
        );
        close.clicked()
    })
    .inner
}

pub fn center(app: &mut App, ctx: &egui::Context, core: &CoreState) {
    if !app.state.notices_open {
        return;
    }
    let Some(offset) = overlay_offset(app, ctx) else {
        return;
    };
    let mut clear = false;
    let area = egui::Area::new(Id::unique("notification_center"))
        .order(egui::Order::Foreground)
        .anchor(Align2::LEFT_BOTTOM, offset)
        .fade_in(false)
        .show(ctx, |ui| {
            let p = theme::palette(ui);
            ui.set_width(360.0);
            egui::Frame::new()
                .fill(p.card)
                .stroke(Stroke::new(1.0, p.border))
                .corner_radius(CornerRadius::same(14))
                .shadow(egui::epaint::Shadow {
                    offset: [0, 14],
                    blur: 40,
                    spread: 0,
                    color: Color32::from_black_alpha(if p.dark { 110 } else { 40 }),
                })
                .show(ui, |ui| {
                    ui.set_width(360.0);
                    egui::Frame::new()
                        .inner_margin(Margin {
                            left: 14,
                            right: 12,
                            top: 8,
                            bottom: 8,
                        })
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new("Notifications")
                                        .font(FontId::new(14.5, theme::bold()))
                                        .color(p.text),
                                );
                                ui.with_layout(
                                    Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if !core.notices.is_empty()
                                            && theme::tiny_button(ui, "Clear").clicked()
                                        {
                                            clear = true;
                                        }
                                    },
                                );
                            });
                        });
                    theme::divider(ui);
                    if core.notices.is_empty() {
                        ui.add_space(18.0);
                        ui.vertical_centered(|ui| {
                            ui.label(
                                RichText::new("No notifications yet")
                                    .size(12.5)
                                    .color(p.secondary),
                            );
                            theme::muted(ui, "Updates and errors show up here.");
                        });
                        ui.add_space(18.0);
                    } else {
                        egui::ScrollArea::vertical()
                            .max_height(360.0)
                            .id_salt("notification_center_scroll")
                            .show(ui, |ui| {
                                for (index, notice) in core.notices.iter().enumerate() {
                                    if index > 0 {
                                        theme::divider(ui);
                                    }
                                    theme::list_row(ui, |ui| {
                                        ui.horizontal(|ui| {
                                            theme::dot(
                                                ui,
                                                level_color(&p, notice.level),
                                            );
                                            ui.vertical(|ui| {
                                                ui.add(
                                                    egui::Label::new(
                                                        RichText::new(&notice.message)
                                                            .size(12.0)
                                                            .color(p.text),
                                                    )
                                                    .wrap(),
                                                );
                                                theme::muted(
                                                    ui,
                                                    &format_age(notice.at),
                                                );
                                            });
                                        });
                                    });
                                }
                            });
                    }
                })
                .response
                .rect
        });

    let popup_rect = area.inner;
    let pointer = ctx.input(|input| input.pointer.interact_pos());
    let clicked = ctx.input(|input| input.pointer.any_click());
    let escape = ctx.input(|input| input.key_pressed(egui::Key::Escape));
    let outside = clicked
        && pointer
            .map(|pos| !popup_rect.contains(pos) && !app.state.bell_rect.contains(pos))
            .unwrap_or(false);

    if clear {
        app.backend.clear_notices();
    }
    if escape || outside {
        app.state.notices_open = false;
    }
}
