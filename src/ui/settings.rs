use eframe::egui;

use crate::service::{CoreState, NoticeLevel};
use crate::ui::{theme, App, ActionDone};
use crate::win::{autostart, elevation};

pub fn show(app: &mut App, ui: &mut egui::Ui, core: &CoreState) {
    let mut cfg = app.backend.config();
    let identity = app.backend.identity();
    let mut dirty = false;
    let p = theme::palette(ui);
    let remote = app.backend.is_remote();

    theme::page_header(
        ui,
        "Settings",
        if remote {
            "Changes apply immediately to the background service."
        } else {
            "Changes apply immediately and are stored on this PC."
        },
    );

    theme::card(ui, |ui| {
        theme::section_title(ui, "Startup");
        ui.add_space(10.0);
        if remote {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new("Windows service")
                            .size(13.0)
                            .color(p.text),
                    );
                    theme::muted(
                        ui,
                        "peerPrint starts with Windows, before anyone signs in, and keeps sharing printers in the background.",
                    );
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let (label, color) = if app.backend.connected() {
                        ("Running", p.success)
                    } else {
                        ("Not reachable", p.danger)
                    };
                    theme::pill(ui, label, color);
                });
            });
        } else {
            ui.horizontal(|ui| {
                let mut enabled = cfg.autostart;
                if theme::toggle(ui, &mut enabled).changed() {
                    cfg.autostart = enabled;
                    match autostart::set_enabled(enabled) {
                        Ok(()) => {
                            dirty = true;
                            app.backend.notice(
                                NoticeLevel::Info,
                                if enabled {
                                    "peerPrint will start with Windows"
                                } else {
                                    "peerPrint will no longer start with Windows"
                                },
                            );
                        }
                        Err(err) => app
                            .backend
                            .notice(NoticeLevel::Error, format!("updating startup: {err:#}")),
                    }
                }
                ui.add_space(10.0);
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new("Start with Windows")
                            .size(13.0)
                            .color(p.text),
                    );
                    theme::muted(
                        ui,
                        "Runs in the background so printers keep working without this window.",
                    );
                });
            });
        }

        ui.add_space(14.0);
        theme::divider(ui);
        ui.add_space(14.0);
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(
                    egui::RichText::new("Keep running in the notification area")
                        .size(13.0)
                        .color(p.text),
                );
                theme::muted(
                    ui,
                    if remote {
                        "Closing this window hides it. The service keeps printing after you quit the tray app."
                    } else {
                        "Closing the window hides it; printers stay available until you quit."
                    },
                );
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let label = if remote {
                    "Quit tray app"
                } else {
                    "Quit peerPrint"
                };
                if theme::danger_button(ui, label).clicked() {
                    app.state.quit_requested = true;
                }
            });
        });
    });

    ui.add_space(16.0);
    theme::card(ui, |ui| {
        theme::section_title(ui, "Network");
        ui.add_space(10.0);
        egui::Grid::new("network_settings")
            .num_columns(2)
            .spacing([24.0, 12.0])
            .show(ui, |ui| {
                ui.label(
                    egui::RichText::new("Host port")
                        .size(12.5)
                        .color(p.secondary),
                );
                if ui
                    .add(egui::DragValue::new(&mut cfg.host.listen_port).range(1024..=65530))
                    .changed()
                {
                    dirty = true;
                }
                ui.end_row();
                ui.label(
                    egui::RichText::new("First relay port")
                        .size(12.5)
                        .color(p.secondary),
                );
                if ui
                    .add(egui::DragValue::new(&mut cfg.client.port_base).range(1024..=65500))
                    .changed()
                {
                    dirty = true;
                }
                ui.end_row();
                ui.label(
                    egui::RichText::new("Outbox limit (MB)")
                        .size(12.5)
                        .color(p.secondary),
                );
                if ui
                    .add(egui::DragValue::new(&mut cfg.client.outbox_limit_mb).range(10..=4096))
                    .changed()
                {
                    dirty = true;
                }
                ui.end_row();
            });

        ui.add_space(14.0);
        theme::divider(ui);
        ui.add_space(14.0);
        let has_rules = app.backend.firewall_rules();
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("Windows Firewall")
                    .size(12.5)
                    .color(p.text),
            );
            theme::pill(
                ui,
                if has_rules { "Allowed" } else { "Not configured" },
                if has_rules { p.success } else { p.warn },
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if !has_rules {
                    if app.state.firewall_busy {
                        ui.spinner();
                    } else if theme::secondary_button(ui, "Add rules").clicked() {
                        app.state.firewall_busy = true;
                        let backend = app.backend.clone();
                        app.spawn_action(ui.ctx(), ActionDone::Firewall(Ok(())), async move {
                            backend
                                .setup_firewall()
                                .await
                                .map_err(|err| format!("{err:#}"))
                        });
                    }
                }
            });
        });
        theme::muted(
            ui,
            "Private-network rules let other PCs discover this host and send jobs.",
        );
    });

    ui.add_space(16.0);
    let update_checking = app.state.updates.checking;
    let update_checked = app.state.updates.checked;
    let update_available = app.state.updates.available.clone();
    let update_error = app.state.updates.error.clone();
    let update_last_check = app.state.updates.last_check.max(cfg.updates.last_check);
    theme::card(ui, |ui| {
        theme::section_title(ui, "Updates");
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            let mut enabled = cfg.updates.auto_check;
            if theme::toggle(ui, &mut enabled).changed() {
                cfg.updates.auto_check = enabled;
                dirty = true;
            }
            ui.add_space(10.0);
            ui.vertical(|ui| {
                ui.label(
                    egui::RichText::new("Check for updates automatically")
                        .size(13.0)
                        .color(p.text),
                );
                theme::muted(
                    ui,
                    "Asks GitHub once a day; only the version number is compared.",
                );
            });
        });

        ui.add_space(14.0);
        theme::divider(ui);
        ui.add_space(14.0);

        ui.horizontal(|ui| {
            // Reserve the button column first so long error text wraps instead of
            // running underneath the buttons.
            let text_width = (ui.available_width() - 240.0).max(180.0);
            ui.vertical(|ui| {
                ui.set_max_width(text_width);
                if let Some(release) = &update_available {
                    ui.label(
                        egui::RichText::new(format!("peerPrint v{} is available", release.version))
                            .size(13.0)
                            .color(p.text),
                    );
                    theme::muted(
                        ui,
                        "Download the setup from the release page and run it; settings and printers are kept.",
                    );
                } else if update_checking {
                    ui.label(egui::RichText::new("Checking GitHub…").size(13.0).color(p.text));
                } else if update_error.is_some() {
                    ui.label(
                        egui::RichText::new("Could not check for updates")
                            .size(13.0)
                            .color(p.text),
                    );
                    if let Some(error) = &update_error {
                        theme::muted(ui, error);
                    }
                } else if update_checked || update_last_check > 0 {
                    ui.label(egui::RichText::new("peerPrint is up to date").size(13.0).color(p.text));
                } else {
                    ui.label(egui::RichText::new("Not checked yet").size(13.0).color(p.text));
                }
                let mut detail = format!("Version {}", env!("CARGO_PKG_VERSION"));
                if update_last_check > 0 {
                    detail.push_str(&format!(
                        " · last checked {}",
                        crate::ui::format_age(update_last_check)
                    ));
                }
                theme::muted(ui, &detail);
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if let Some(release) = update_available.clone() {
                    if theme::primary_button(ui, "View release").clicked() {
                        if let Err(err) = crate::update::open_release_page(&release) {
                            app.backend.notice(
                                NoticeLevel::Error,
                                format!("opening the release page: {err:#}"),
                            );
                        }
                    }
                }
                if update_checking {
                    ui.spinner();
                } else if theme::secondary_button(ui, "Check now").clicked() {
                    app.start_update_check(ui.ctx());
                }
            });
        });
    });

    ui.add_space(16.0);
    theme::card(ui, |ui| {
        theme::section_title(ui, "Diagnostics");
        ui.add_space(10.0);
        egui::Grid::new("diagnostics")
            .num_columns(2)
            .spacing([24.0, 8.0])
            .show(ui, |ui| {
                theme::muted(ui, "Version");
                ui.label(egui::RichText::new(env!("CARGO_PKG_VERSION")).size(12.0));
                ui.end_row();
                theme::muted(ui, "Engine");
                ui.label(
                    egui::RichText::new(if remote {
                        if app.backend.connected() {
                            "Background service"
                        } else {
                            "Not reachable"
                        }
                    } else if elevation::is_elevated() {
                        "In this process (administrator)"
                    } else {
                        "In this process"
                    })
                    .size(12.0),
                );
                ui.end_row();
                if let Some(role) = match cfg.role {
                    crate::config::Role::Host => Some("Host only"),
                    crate::config::Role::Client => Some("Client only"),
                    crate::config::Role::Both => None,
                } {
                    theme::muted(ui, "Install role");
                    ui.label(egui::RichText::new(role).size(12.0));
                    ui.end_row();
                }
                theme::muted(ui, "Host ID");
                ui.label(
                    egui::RichText::new(&identity.host_id)
                        .monospace()
                        .size(11.5),
                );
                ui.end_row();
                theme::muted(ui, "Certificate");
                ui.label(
                    egui::RichText::new(&identity.tls_fp)
                        .monospace()
                        .size(11.0),
                );
                ui.end_row();
                theme::muted(ui, "Data folder");
                ui.label(
                    egui::RichText::new(app.backend.data_dir())
                        .monospace()
                        .size(11.0),
                );
                ui.end_row();
            });

        if crate::win::spooler::protected_print_mode() {
            ui.add_space(10.0);
            theme::inline_warning(
                ui,
                "Windows protected print mode is on. Windows blocks third-party printer drivers, \
                 so peerPrint queues cannot be installed until it is turned off in Windows Settings.",
                |_ui| {},
            );
        }

        ui.add_space(14.0);
        ui.horizontal(|ui| {
            if theme::secondary_button(ui, "Copy diagnostics").clicked() {
                let report = serde_json::json!({
                    "version": env!("CARGO_PKG_VERSION"),
                    "data_dir": app.backend.data_dir(),
                    "service": remote,
                    "service_connected": app.backend.connected(),
                    "host_id": identity.host_id,
                    "tls_fingerprint": identity.tls_fp,
                    "elevated": elevation::is_elevated(),
                    "host_running": core.host_running,
                    "host_error": core.host_error,
                    "firewall_rules": app.backend.firewall_rules(),
                    "protected_print_mode": crate::win::spooler::protected_print_mode(),
                    "discovered_hosts": core.discovered.len(),
                    "saved_printers": cfg.client.saved.len(),
                    "relays": core.relays.iter().map(|relay| serde_json::json!({
                        "queue": relay.queue_name,
                        "port": relay.port,
                        "listening": relay.listening,
                        "last_error": relay.last_error,
                    })).collect::<Vec<_>>(),
                });
                ui.ctx()
                    .copy_text(serde_json::to_string_pretty(&report).unwrap_or_default());
                app.backend.notice(NoticeLevel::Info, "Diagnostics copied");
            }
            if theme::secondary_button(ui, "Open data folder").clicked() {
                let _ = std::process::Command::new("explorer.exe")
                    .arg(app.backend.data_dir())
                    .spawn();
            }
        });
    });

    if dirty {
        if let Err(err) = app.backend.set_config(cfg) {
            app.backend
                .notice(NoticeLevel::Error, format!("saving settings: {err:#}"));
        }
    }
}
