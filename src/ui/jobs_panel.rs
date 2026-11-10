use std::sync::Arc;

use eframe::egui;

use crate::backend::Backend;
use crate::service::{CoreState, JobDirection, NoticeLevel};
use crate::ui::{format_age, format_bytes, theme};

pub fn show(
    ui: &mut egui::Ui,
    backend: &Arc<Backend>,
    core: &CoreState,
    filter: Option<JobDirection>,
) {
    let p = theme::palette(ui);
    let jobs: Vec<_> = core
        .jobs
        .iter()
        .filter(|job| filter.map(|direction| job.direction == direction).unwrap_or(true))
        .collect();
    if jobs.is_empty() {
        ui.add_space(16.0);
        ui.vertical_centered(|ui| {
            ui.label(
                egui::RichText::new("No jobs yet")
                    .size(12.5)
                    .color(p.secondary),
            );
            theme::muted(ui, "Jobs appear here as documents move between PCs.");
        });
        ui.add_space(16.0);
        return;
    }

    for (index, job) in jobs.iter().enumerate() {
        if index > 0 {
            theme::divider(ui);
        }
        theme::list_row(ui, |ui| {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    let doc = if job.doc_name.chars().count() > 44 {
                        format!("{}…", job.doc_name.chars().take(44).collect::<String>())
                    } else {
                        job.doc_name.clone()
                    };
                    let doc_label = ui.label(
                        egui::RichText::new(doc)
                            .size(12.5)
                            .color(p.text),
                    );
                    if let Some(error) = &job.error {
                        doc_label.on_hover_text(error);
                    }
                    let pages = if job.pages > 0 {
                        format!("{} pages", job.pages)
                    } else {
                        "pages unknown".to_string()
                    };
                    theme::muted(
                        ui,
                        &format!(
                            "{} · {} · {} · {} · {}",
                            job.printer_name,
                            job.peer,
                            format_bytes(job.bytes),
                            pages,
                            format_age(job.created)
                        ),
                    );
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    theme::pill(ui, &job.state, theme::state_color(&p, &job.state));
                    ui.add_space(2.0);
                    cancel_button(ui, backend, job);
                    if filter.is_none() {
                        let (label, color) = match job.direction {
                            JobDirection::Host => ("Received", p.info),
                            JobDirection::Client => ("Sent", p.secondary),
                        };
                        ui.add_space(2.0);
                        theme::pill(ui, label, color);
                    }
                });
            });
        });
    }
}

fn cancel_button(ui: &mut egui::Ui, backend: &Arc<Backend>, job: &crate::service::JobRecord) {
    match job.direction {
        JobDirection::Host => {
            if matches!(job.state.as_str(), "Receiving" | "Spooling" | "Printing")
                && theme::tiny_button(ui, "Cancel")
                    .on_hover_text("Stop this job in the Windows print queue")
                    .clicked()
            {
                backend.request_cancel(job.id);
                backend.notice(
                    NoticeLevel::Info,
                    format!("Cancelling '{}'…", job.doc_name),
                );
            }
        }
        JobDirection::Client => {
            if matches!(
                job.state.as_str(),
                "Queued" | "Waiting for host" | "Uploading" | "Printing"
            ) {
                let hint = if job.state == "Printing" {
                    "Ask the host PC to stop printing this job"
                } else {
                    "Remove this job from the outbox"
                };
                if theme::tiny_button(ui, "Cancel")
                    .on_hover_text(hint)
                    .clicked()
                {
                    backend.cancel_client_job(job);
                }
            }
        }
    }
}
