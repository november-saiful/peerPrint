use eframe::egui::{self, CornerRadius, CursorIcon, RichText, Stroke, Vec2};

use crate::service::JobDirection;
use crate::ui::{theme, App};

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let core = app.backend.core();
    let cfg = app.backend.config();

    if !cfg.role.hosts() && app.state.history_filter == Some(JobDirection::Host) {
        app.state.history_filter = None;
    }
    if !cfg.role.clients() && app.state.history_filter == Some(JobDirection::Client) {
        app.state.history_filter = None;
    }

    theme::page_header(
        ui,
        "History",
        "Every document peerPrint has handled on this PC.",
    );

    ui.horizontal(|ui| {
        filter_chip(ui, &mut app.state.history_filter, None, "All jobs");
        if cfg.role.hosts() {
            filter_chip(
                ui,
                &mut app.state.history_filter,
                Some(JobDirection::Host),
                "Received",
            );
        }
        if cfg.role.clients() {
            filter_chip(
                ui,
                &mut app.state.history_filter,
                Some(JobDirection::Client),
                "Sent",
            );
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let total = core.jobs.len();
            theme::muted(ui, &format!("{total} job{} kept", if total == 1 { "" } else { "s" }));
        });
    });

    ui.add_space(12.0);
    theme::list_card(ui, |ui| {
        ui.add_space(4.0);
        crate::ui::jobs_panel::show(ui, &app.backend, &core, app.state.history_filter);
        ui.add_space(4.0);
    });
}

fn filter_chip(
    ui: &mut egui::Ui,
    selected: &mut Option<JobDirection>,
    value: Option<JobDirection>,
    label: &str,
) {
    let p = theme::palette(ui);
    let active = *selected == value;
    let response = ui
        .add(
            egui::Button::new(
                RichText::new(label).size(12.0).color(if active {
                    p.accent_text
                } else {
                    p.secondary
                }),
            )
            .fill(if active { p.accent_soft } else { p.card })
            .stroke(Stroke::new(
                1.0,
                if active { p.accent } else { p.border },
            ))
            .corner_radius(CornerRadius::same(99))
            .min_size(Vec2::new(0.0, 26.0)),
        )
        .on_hover_cursor(CursorIcon::PointingHand);
    if response.clicked() {
        *selected = value;
    }
}
