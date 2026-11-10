use eframe::egui;
use uuid::Uuid;

use crate::config::SharedPrinter;
use crate::service::NoticeLevel;
use crate::ui::{format_age, theme, App, ActionDone};

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let core = app.backend.core();
    let mut cfg = app.backend.config();
    let mut dirty = false;
    let mut notice: Option<(NoticeLevel, String)> = None;
    let p = theme::palette(ui);

    theme::page_header(
        ui,
        "Host",
        "Share a USB printer connected to this PC with the other PCs on your network.",
    );

    egui::Frame::new()
        .fill(p.card)
        .stroke(egui::Stroke::new(1.0, p.border))
        .corner_radius(14)
        .inner_margin(egui::Margin::same(18))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let mut enabled = cfg.host.enabled;
                if theme::toggle(ui, &mut enabled).changed() {
                    cfg.host.enabled = enabled;
                    dirty = true;
                }
                ui.add_space(10.0);
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new("Share my printers")
                            .size(13.5)
                            .color(p.text),
                    );
                    let (text, color) = if core.host_running {
                        (
                            "Running and discoverable on this network".to_string(),
                            p.success,
                        )
                    } else if cfg.host.enabled && cfg.host.shared.is_empty() {
                        ("Choose at least one printer below".to_string(), p.warn)
                    } else if let Some(error) = &core.host_error {
                        (error.clone(), p.danger)
                    } else if cfg.host.enabled {
                        ("Starting…".to_string(), p.info)
                    } else {
                        ("Off — nothing is shared".to_string(), p.muted)
                    };
                    ui.label(egui::RichText::new(text).size(11.5).color(color));
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let (label, color) = if core.host_running {
                        ("Hosting", p.success)
                    } else if cfg.host.enabled {
                        ("Enabled", p.warn)
                    } else {
                        ("Off", p.muted)
                    };
                    theme::pill(ui, label, color);
                });
            });

            if cfg.host.enabled {
                ui.add_space(14.0);
                theme::divider(ui);
                ui.add_space(14.0);
                egui::Grid::new("host_identity")
                    .num_columns(2)
                    .spacing([18.0, 12.0])
                    .show(ui, |ui| {
                        ui.label(
                            egui::RichText::new("Name shown to others")
                                .size(12.5)
                                .color(p.secondary),
                        );
                        if theme::text_input(ui, &mut cfg.host.display_name, 220.0, "This PC")
                            .changed()
                        {
                            dirty = true;
                        }
                        ui.end_row();
                        ui.label(egui::RichText::new("Host port").size(12.5).color(p.secondary));
                        if ui
                            .add(egui::DragValue::new(&mut cfg.host.listen_port).range(1024..=65530))
                            .changed()
                        {
                            dirty = true;
                        }
                        ui.end_row();
                    });

                ui.add_space(12.0);
                let mut require_pin = cfg.host.require_pin;
                ui.horizontal(|ui| {
                    if theme::toggle(ui, &mut require_pin).changed() {
                        cfg.host.require_pin = require_pin;
                        dirty = true;
                    }
                    ui.add_space(10.0);
                    ui.label(
                        egui::RichText::new("Require a PIN to pair")
                            .size(12.5)
                            .color(p.text),
                    );
                    if cfg.host.require_pin {
                        ui.add_space(8.0);
                        theme::password_input(ui, &mut app.state.pin_edit, 120.0, "4–12 digits");
                        if theme::secondary_button(ui, "Set PIN").clicked() {
                            let pin = app.state.pin_edit.trim().to_string();
                            if pin.len() < 4
                                || pin.len() > 12
                                || !pin.chars().all(|c| c.is_ascii_digit())
                            {
                                notice = Some((
                                    NoticeLevel::Warn,
                                    "PIN must be 4–12 digits".to_string(),
                                ));
                            } else {
                                match cfg.set_host_pin(Some(&pin)) {
                                    Ok(()) => {
                                        app.state.pin_edit.clear();
                                        dirty = true;
                                        notice =
                                            Some((NoticeLevel::Info, "PIN updated".to_string()));
                                    }
                                    Err(err) => {
                                        notice = Some((
                                            NoticeLevel::Error,
                                            format!("setting PIN: {err:#}"),
                                        ))
                                    }
                                }
                            }
                        }
                        if cfg.host.pin_dpapi.is_some() {
                            if theme::secondary_button(ui, "Clear").clicked() {
                                match cfg.set_host_pin(None) {
                                    Ok(()) => {
                                        cfg.host.require_pin = false;
                                        dirty = true;
                                        notice =
                                            Some((NoticeLevel::Info, "PIN cleared".to_string()));
                                    }
                                    Err(err) => {
                                        notice = Some((
                                            NoticeLevel::Error,
                                            format!("clearing PIN: {err:#}"),
                                        ))
                                    }
                                }
                            }
                            theme::pill(ui, "PIN set", p.success);
                        } else {
                            theme::pill(ui, "No PIN yet", p.warn);
                        }
                    }
                });
                theme::muted(
                    ui,
                    "Clients enter this PIN once when they add your printer.",
                );
            }
        });

    if cfg.host.enabled && !cfg.host.firewall_configured {
        ui.add_space(10.0);
        theme::inline_warning(
            ui,
            "Windows Firewall may block other PCs from finding this host.",
            |ui| {
                if app.state.firewall_busy {
                    ui.spinner();
                } else if theme::secondary_button(ui, "Fix").clicked() {
                    app.state.firewall_busy = true;
                    let backend = app.backend.clone();
                    app.spawn_action(ui.ctx(), ActionDone::Firewall(Ok(())), async move {
                        backend
                            .setup_firewall()
                            .await
                            .map_err(|err| format!("{err:#}"))
                    });
                }
            },
        );
    }

    ui.add_space(22.0);
    ui.horizontal(|ui| {
        theme::section_title(ui, "Printers on this PC");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if theme::secondary_button(ui, "Refresh").clicked() {
                app.backend.refresh_local_printers();
            }
        });
    });
    ui.add_space(8.0);

    let mut toggled: Option<(bool, SharedPrinter, String)> = None;
    theme::list_card(ui, |ui| {
        if core.local_printers.is_empty() {
            theme::list_row(ui, |ui| {
                ui.label(
                    egui::RichText::new("No USB printers detected.")
                        .size(12.5)
                        .color(p.secondary),
                );
                theme::muted(ui, "Connect a printer, then choose Refresh.");
            });
        } else {
            for (index, printer) in core.local_printers.iter().enumerate() {
                if index > 0 {
                    theme::divider(ui);
                }
                theme::list_row(ui, |ui| {
                    let existing = cfg
                        .host
                        .shared
                        .iter()
                        .position(|shared| shared.spooler_name == printer.name);
                    let mut enabled = existing.is_some();
                    let mut name = existing
                        .map(|position| cfg.host.shared[position].display_name.clone())
                        .unwrap_or_else(|| printer.name.clone());
                    ui.horizontal(|ui| {
                        let mut on = enabled;
                        ui.add_enabled_ui(printer.shareable, |ui| {
                            theme::toggle(ui, &mut on);
                        });
                        enabled = on;
                        ui.add_space(10.0);
                        ui.vertical(|ui| {
                            ui.label(egui::RichText::new(&printer.name).size(13.0).color(p.text));
                            ui.horizontal(|ui| {
                                theme::dot(ui, if printer.online { p.success } else { p.warn });
                                let jobs = if printer.jobs == 1 { "1 job" } else { "jobs" };
                                theme::muted(
                                    ui,
                                    &format!(
                                        "{} · {} · {} {}",
                                        printer.driver_name,
                                        printer.status_text,
                                        printer.jobs,
                                        jobs
                                    ),
                                );
                            });
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let (label, color) = if !printer.shareable {
                                ("Not shareable", p.muted)
                            } else if printer.online {
                                ("Online", p.success)
                            } else {
                                ("Offline", p.warn)
                            };
                            theme::pill(ui, label, color);
                        });
                    });
                    if printer.shareable && enabled {
                        ui.add_space(10.0);
                        ui.horizontal(|ui| {
                            ui.add_space(48.0);
                            theme::label(ui, "Shown to clients as");
                            if theme::text_input(ui, &mut name, 220.0, &printer.name).changed() {
                                if let Some(position) = existing {
                                    cfg.host.shared[position].display_name = name.clone();
                                    dirty = true;
                                }
                            }
                        });
                    }
                    if printer.shareable && enabled != existing.is_some() {
                        let shared = existing
                            .map(|position| cfg.host.shared[position].clone())
                            .unwrap_or_else(|| SharedPrinter {
                                id: Uuid::new_v4(),
                                spooler_name: printer.name.clone(),
                                display_name: printer.name.clone(),
                                model: printer.driver_name.clone(),
                                driver_name: printer.driver_name.clone(),
                                driver_env: printer.driver_env.clone(),
                                driver_version: printer.driver_version,
                                port_name: printer.port_name.clone(),
                                enabled: true,
                            });
                        toggled = Some((enabled, shared, printer.name.clone()));
                    }
                });
            }
        }
    });

    let unshareable: Vec<_> = core
        .local_printers
        .iter()
        .filter(|printer| !printer.shareable)
        .map(|printer| printer.name.clone())
        .collect();
    if !unshareable.is_empty() {
        ui.add_space(6.0);
        theme::muted(
            ui,
            &format!(
                "Windows protected print mode blocks: {}",
                unshareable.join(", ")
            ),
        );
    }

    if let Some((enabled, shared, spooler)) = toggled {
        if enabled {
            cfg.host.shared.push(shared);
            notice = Some((NoticeLevel::Info, format!("Sharing '{spooler}'")));
        } else {
            cfg
                .host
                .shared
                .retain(|item| item.spooler_name != spooler);
            notice = Some((NoticeLevel::Info, format!("Stopped sharing '{spooler}'")));
        }
        dirty = true;
    }

    ui.add_space(22.0);
    paired_clients(app, ui);

    if dirty {
        if core.local_printers.is_empty() && cfg.host.shared.is_empty() {
            cfg.host.enabled = false;
        }
        if let Err(err) = app.backend.set_config(cfg) {
            app.backend
                .notice(NoticeLevel::Error, format!("saving settings: {err:#}"));
        }
    }
    if let Some((level, message)) = notice {
        app.backend.notice(level, message);
    }
}

fn paired_clients(app: &mut App, ui: &mut egui::Ui) {
    let p = theme::palette(ui);
    let clients = app.backend.paired_clients();
    ui.horizontal(|ui| {
        theme::section_title(ui, "Paired PCs");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            theme::muted(ui, &format!("{} paired", clients.len()));
        });
    });
    ui.add_space(8.0);

    if clients.is_empty() {
        theme::empty_card(
            ui,
            "Nobody has paired with this host yet.",
            "When someone adds one of your printers, their PC appears here.",
        );
        return;
    }

    let mut revoke: Option<Uuid> = None;
    theme::list_card(ui, |ui| {
        for (index, client) in clients.iter().enumerate() {
            if index > 0 {
                theme::divider(ui);
            }
            theme::list_row(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new(&client.name).size(13.0).color(p.text));
                        theme::muted(
                            ui,
                            &format!(
                                "Paired {} · last seen {}",
                                format_age(client.created_at),
                                format_age(client.last_seen)
                            ),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if client.revoked {
                            theme::pill(ui, "Revoked", p.muted);
                        } else if theme::danger_button(ui, "Revoke").clicked() {
                            revoke = Some(client.id);
                        }
                    });
                });
            });
        }
    });

    if let Some(id) = revoke {
        match app.backend.revoke_client(id) {
            Ok(true) => app
                .backend
                .notice(NoticeLevel::Info, "Access revoked for that PC"),
            Ok(false) => {}
            Err(err) => app
                .backend
                .notice(NoticeLevel::Error, format!("revoking access: {err:#}")),
        }
    }
}
