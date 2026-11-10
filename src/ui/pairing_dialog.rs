use eframe::egui;

use crate::service::NoticeLevel;
use crate::ui::{theme, App, ActionDone};

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let Some(mut dialog) = app.state.pair.take() else {
        return;
    };
    let mut keep = true;
    let mut installed = false;
    let ctx = ui.ctx().clone();
    let p = theme::palette(ui);
    let frame = theme::dialog_frame(ui);
    let install_hint = if app.backend.is_remote() {
        "Installing… the printer will be available to everyone on this PC."
    } else {
        "Installing… Windows will ask for administrator permission."
    };

    let modal = egui::Modal::new(egui::Id::unique("pair_printer"))
        .backdrop_color(egui::Color32::from_black_alpha(if p.dark { 150 } else { 70 }))
        .frame(frame)
        .show(&ctx, |ui| {
            ui.set_width(420.0);
            ui.label(
                egui::RichText::new(&dialog.printer_name)
                    .font(egui::FontId::new(19.0, theme::bold()))
                    .color(p.text),
            );
            theme::muted(ui, &format!("Shared by {}", dialog.host.display_name));
            ui.add_space(16.0);

            egui::Frame::new()
                .fill(p.faint)
                .corner_radius(10)
                .inner_margin(egui::Margin::symmetric(12, 10))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        theme::label(ui, "Host certificate fingerprint");
                        ui.with_layout(
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                if theme::tiny_button(ui, "Copy").clicked() {
                                    ui.ctx().copy_text(dialog.host.tls_fp.clone());
                                }
                            },
                        );
                    });
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(&dialog.host.tls_fp)
                                .monospace()
                                .size(10.5)
                                .color(p.secondary),
                        )
                        .wrap(),
                    );
                });
            ui.add_space(6.0);
            theme::muted(
                ui,
                "Compare this with the fingerprint shown on the host PC to rule out an impostor.",
            );
            ui.add_space(14.0);

            if dialog.host.pin_required {
                ui.label(
                    egui::RichText::new("PIN shown on the host PC")
                        .size(12.5)
                        .color(p.text),
                );
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    theme::password_input(ui, &mut dialog.pin, 150.0, "PIN");
                    theme::muted(ui, "Required to pair with this host.");
                });
            } else {
                ui.horizontal(|ui| {
                    theme::dot(ui, p.success);
                    theme::muted(ui, "This host does not require a PIN.");
                });
            }

            if dialog.busy {
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.spinner();
                    theme::muted(ui, install_hint);
                });
            }

            if let Some(error) = &dialog.error {
                ui.add_space(10.0);
                theme::notice(ui, NoticeLevel::Error, error);
            }

            ui.add_space(18.0);
            theme::divider(ui);
            ui.add_space(14.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let can_install = !dialog.busy
                        && (!dialog.host.pin_required || !dialog.pin.trim().is_empty());
                    let install = ui
                        .add_enabled_ui(can_install, |ui| theme::primary_button(ui, "Install printer"))
                        .inner;
                    if install.clicked() {
                        dialog.busy = true;
                        dialog.error = None;
                        installed = true;
                    }
                    let cancel = ui
                        .add_enabled_ui(!dialog.busy, |ui| theme::secondary_button(ui, "Cancel"))
                        .inner;
                    if cancel.clicked() {
                        keep = false;
                    }
                });
            });
        });

    if modal.should_close() && !dialog.busy {
        keep = false;
    }

    if installed {
        let host = dialog.host.clone();
        let printer_id = dialog.printer_id;
        let pin = dialog.pin.trim().to_string();
        let backend = app.backend.clone();
        app.spawn_action(
            &ctx,
            ActionDone::Installed(printer_id, Ok(())),
            async move {
                backend
                    .pair(&host, printer_id, &pin)
                    .await
                    .map(|_| ())
                    .map_err(|err| format!("{err:#}"))
            },
        );
    }
    if keep {
        app.state.pair = Some(dialog);
    }
}
