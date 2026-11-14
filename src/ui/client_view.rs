use std::net::SocketAddr;

use eframe::egui;

use crate::ui::{theme, App, ActionDone, PairingDialog};

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    crate::ui::pairing_dialog::show(app, ui);

    let core = app.backend.core();
    let cfg = app.backend.config();
    let p = theme::palette(ui);

    theme::page_header(
        ui,
        "Client",
        "Add a printer shared by another PC, then print to it like a local printer.",
    );

    ui.horizontal(|ui| {
        theme::section_title(ui, "Found on this network");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if !core.discovered.is_empty() {
                let hosts = core.discovered.len();
                theme::muted(
                    ui,
                    &format!("{hosts} host{}", if hosts == 1 { "" } else { "s" }),
                );
            }
        });
    });
    ui.add_space(8.0);

    if core.discovered.is_empty() {
        theme::empty_card(
            ui,
            "No shared printers found yet.",
            "Make sure the host PC is sharing and both PCs are on the same network. If discovery is blocked, add the host by address below.",
        );
    } else {
        for host in &core.discovered {
            theme::list_card(ui, |ui| {
                theme::list_row(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.label(
                                egui::RichText::new(&host.display_name)
                                    .size(13.0)
                                    .color(p.text),
                            );
                            theme::muted(ui, &host.addr);
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if host.pin_required {
                                theme::pill(ui, "PIN required", p.warn);
                            }
                        });
                    });
                });
                for printer in &host.printers {
                    theme::divider(ui);
                    theme::list_row(ui, |ui| {
                        ui.horizontal(|ui| {
                            theme::dot(ui, if printer.online { p.success } else { p.warn });
                            ui.vertical(|ui| {
                                ui.label(
                                    egui::RichText::new(&printer.name)
                                        .size(12.5)
                                        .color(p.text),
                                );
                                theme::muted(
                                    ui,
                                    &format!("{} · {}", printer.model, printer.status),
                                );
                            });
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    let saved = cfg.client.saved.iter().any(|saved| {
                                        saved.host_id == host.host_id.to_string()
                                            && saved.printer_id == printer.id
                                    });
                                    let busy = app
                                        .state
                                        .pair
                                        .as_ref()
                                        .map(|dialog| dialog.busy)
                                        .unwrap_or(false);
                                    if saved {
                                        theme::pill(ui, "Installed", p.success);
                                    } else if busy {
                                        ui.add_enabled_ui(false, |ui| {
                                            theme::secondary_button(ui, "Add");
                                        });
                                    } else if theme::primary_button(ui, "Add").clicked() {
                                        app.state.pair = Some(PairingDialog {
                                            host: host.clone(),
                                            printer_id: printer.id,
                                            printer_name: printer.name.clone(),
                                            pin: String::new(),
                                            error: None,
                                            busy: false,
                                        });
                                    }
                                },
                            );
                        });
                    });
                }
            });
            ui.add_space(8.0);
        }
    }

    ui.add_space(14.0);
    theme::section_title(ui, "Add by address");
    ui.add_space(8.0);
    theme::card(ui, |ui| {
        ui.horizontal(|ui| {
            theme::text_input(ui, &mut app.state.manual_addr, 220.0, "192.168.1.20:39100");
            if app.state.probe_busy {
                ui.spinner();
                theme::muted(ui, "Looking for the host…");
            } else if theme::primary_button(ui, "Find host").clicked() {
                match parse_manual(&app.state.manual_addr) {
                    Some(addr) => {
                        app.state.probe_busy = true;
                        let backend = app.backend.clone();
                        app.spawn_action(ui.ctx(), ActionDone::Probed(Ok(())), async move {
                            backend
                                .probe(addr)
                                .await
                                .map_err(|err| format!("{err:#}"))
                        });
                    }
                    None => {
                        app.backend.notice(
                            crate::service::NoticeLevel::Warn,
                            "Enter an address like 192.168.1.20:39100",
                        );
                    }
                }
            }
        });
        theme::muted(
            ui,
            "Use this when mDNS discovery is blocked (AP isolation, VPN).",
        );
    });

    ui.add_space(22.0);
    theme::section_title(ui, "Installed printers");
    ui.add_space(8.0);
    if cfg.client.saved.is_empty() {
        theme::empty_card(
            ui,
            "No peerPrint printers installed yet.",
            "Pick a shared printer above and choose Add.",
        );
    } else {
        theme::list_card(ui, |ui| {
            for (index, saved) in cfg.client.saved.iter().enumerate() {
                if index > 0 {
                    theme::divider(ui);
                }
                let relay = core.relays.iter().find(|relay| relay.saved_id == saved.id);
                theme::list_row(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.label(
                                egui::RichText::new(&saved.queue_name)
                                    .size(13.0)
                                    .color(p.text),
                            );
                            theme::muted(
                                ui,
                                &format!(
                                    "{} · {} · {}",
                                    saved.printer_model, saved.host_display, saved.host_addr
                                ),
                            );
                        });
                        ui.with_layout(
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                let removing = app.state.removing == Some(saved.id);
                                if removing {
                                    ui.spinner();
                                } else if theme::danger_button(ui, "Remove").clicked() {
                                    app.state.removing = Some(saved.id);
                                    let backend = app.backend.clone();
                                    let id = saved.id;
                                    app.spawn_action(
                                        ui.ctx(),
                                        ActionDone::Removed(id, Ok(())),
                                        async move {
                                            backend
                                                .remove_printer(id)
                                                .await
                                                .map_err(|err| format!("{err:#}"))
                                        },
                                    );
                                }
                                let repairing = app.state.repairing == Some(saved.id);
                                if repairing {
                                    ui.spinner();
                                } else if relay.is_some_and(|relay| relay.needs_processor_repair)
                                    && theme::secondary_button(ui, "Repair queue").clicked()
                                {
                                    app.state.repairing = Some(saved.id);
                                    let backend = app.backend.clone();
                                    let id = saved.id;
                                    app.spawn_action(
                                        ui.ctx(),
                                        ActionDone::Repaired(id, Ok(())),
                                        async move {
                                            backend
                                                .repair_printer(id)
                                                .await
                                                .map_err(|err| format!("{err:#}"))
                                        },
                                    );
                                }
                                let testing = app.state.testing == Some(saved.id);
                                if testing {
                                    ui.spinner();
                                } else if theme::secondary_button(ui, "Test page").clicked() {
                                    app.state.testing = Some(saved.id);
                                    let queue = saved.queue_name.clone();
                                    let id = saved.id;
                                    let backend = app.backend.clone();
                                    app.spawn_action(
                                        ui.ctx(),
                                        ActionDone::Tested(id, Ok(())),
                                        async move {
                                            backend
                                                .test_page(queue)
                                                .await
                                                .map_err(|err| format!("{err:#}"))
                                        },
                                    );
                                }
                            },
                        );
                    });
                    ui.add_space(6.0);
                    let (color, text) = match relay {
                        Some(relay) if relay.listening => (
                            p.success,
                            format!("Relay listening on 127.0.0.1:{}", relay.port),
                        ),
                        Some(relay) => (
                            p.danger,
                            relay
                                .last_error
                                .clone()
                                .unwrap_or_else(|| "Relay not running".to_string()),
                        ),
                        None => (p.muted, "Relay starting…".to_string()),
                    };
                    let broken = relay.is_some_and(|relay| !relay.listening);
                    ui.horizontal(|ui| {
                        theme::dot(ui, color);
                        ui.label(
                            egui::RichText::new(&text)
                                .size(11.5)
                                .color(if broken { p.danger } else { p.muted }),
                        );
                        if let Some(job) = relay.and_then(|relay| relay.last_job.as_ref()) {
                            theme::muted(ui, &format!("· {job}"));
                        }
                    });
                    if relay.is_some_and(|relay| !relay.listening && relay.last_error.is_some()) {
                        theme::muted(
                            ui,
                            "Queued jobs stay on this PC until peerPrint can reach the host.",
                        );
                    }
                    if relay.is_some_and(|relay| relay.needs_processor_repair) {
                        theme::muted(
                            ui,
                            "This queue uses the default print processor, so manual two-sided printing cannot prompt you to flip the page.",
                        );
                    }
                });
            }
        });
    }

}

fn parse_manual(text: &str) -> Option<SocketAddr> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    if let Ok(addr) = text.parse::<SocketAddr>() {
        return Some(addr);
    }
    if text.contains(':') {
        return None;
    }
    format!("{text}:39100").parse().ok()
}
