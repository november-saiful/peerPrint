pub mod client_view;
pub mod history_view;
pub mod host_view;
pub mod jobs_panel;
pub mod notifications;
pub mod pairing_dialog;
pub mod settings;
pub mod theme;
pub mod tray;

use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;

use eframe::egui;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use uuid::Uuid;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    IsIconic, SetForegroundWindow, ShowWindow, SW_RESTORE,
};

use crate::backend::Backend;
use crate::service::NoticeLevel;
use crate::update;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Host,
    Client,
    History,
    Settings,
}

pub struct PairingDialog {
    pub host: crate::service::DiscoveredHost,
    pub printer_id: Uuid,
    pub printer_name: String,
    pub pin: String,
    pub error: Option<String>,
    pub busy: bool,
}

pub struct UiState {
    pub tab: Tab,
    pub manual_addr: String,
    pub probe_busy: bool,
    pub pair: Option<PairingDialog>,
    pub removing: Option<Uuid>,
    pub testing: Option<Uuid>,
    pub pin_edit: String,
    pub firewall_busy: bool,
    pub quit_requested: bool,
    pub history_filter: Option<crate::service::JobDirection>,
    pub notices_open: bool,
    pub notices_ready: bool,
    pub seen_notice: u64,
    pub flyout_notice: u64,
    pub flyouts: Vec<notifications::Flyout>,
    pub bell_rect: egui::Rect,
    pub updates: UpdateState,
}

#[derive(Default)]
pub struct UpdateState {
    pub checking: bool,
    /// At least one check finished in this session.
    pub checked: bool,
    pub available: Option<update::Release>,
    pub error: Option<String>,
    pub last_check: i64,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            tab: Tab::Host,
            manual_addr: String::new(),
            probe_busy: false,
            pair: None,
            removing: None,
            testing: None,
            pin_edit: String::new(),
            firewall_busy: false,
            quit_requested: false,
            history_filter: None,
            notices_open: false,
            notices_ready: false,
            seen_notice: 0,
            flyout_notice: 0,
            flyouts: Vec::new(),
            bell_rect: egui::Rect::NOTHING,
            updates: UpdateState::default(),
        }
    }
}

pub enum ActionDone {
    Probed(Result<(), String>),
    Installed(Uuid, Result<(), String>),
    Removed(Uuid, Result<(), String>),
    Tested(Uuid, Result<(), String>),
    Firewall(Result<(), String>),
    UpdateChecked(Result<Option<update::Release>, String>),
}

pub struct App {
    pub backend: Arc<Backend>,
    pub state: UiState,
    sender: Sender<ActionDone>,
    receiver: Receiver<ActionDone>,
    tray: Option<tray::Tray>,
    tray_rx: Option<Receiver<tray::TrayAction>>,
    tray_attempted: bool,
    quitting: bool,
    start_hidden: bool,
    hide_done: bool,
    close_handled: bool,
    window_theme: Option<bool>,
}

impl App {
    pub fn new(backend: Arc<Backend>, start_hidden: bool) -> Self {
        let (sender, receiver) = channel();
        Self {
            backend,
            state: UiState::default(),
            sender,
            receiver,
            tray: None,
            tray_rx: None,
            tray_attempted: false,
            quitting: false,
            start_hidden,
            hide_done: false,
            close_handled: false,
            window_theme: None,
        }
    }

    /// Runs an action on the service runtime and reports completion on the next frame.
    pub fn spawn_action<F>(&self, ctx: &egui::Context, done: ActionDone, task: F)
    where
        F: std::future::Future<Output = Result<(), String>> + Send + 'static,
    {
        let sender = self.sender.clone();
        let ctx = ctx.clone();
        self.backend.runtime().spawn(async move {
            let result = task.await;
            let _ = sender.send(match done {
                ActionDone::Probed(_) => ActionDone::Probed(result),
                ActionDone::Installed(id, _) => ActionDone::Installed(id, result),
                ActionDone::Removed(id, _) => ActionDone::Removed(id, result),
                ActionDone::Tested(id, _) => ActionDone::Tested(id, result),
                ActionDone::Firewall(_) => ActionDone::Firewall(result),
                // Update checks deliver a release, so they spawn their own task.
                ActionDone::UpdateChecked(_) => unreachable!("update checks are spawned directly"),
            });
            ctx.request_repaint();
        });
    }

    fn drain_actions(&mut self) {
        while let Ok(done) = self.receiver.try_recv() {
            match done {
                ActionDone::Probed(result) => {
                    self.state.probe_busy = false;
                    if let Err(message) = result {
                        self.backend.notice(NoticeLevel::Error, message);
                    }
                }
                ActionDone::Installed(printer_id, result) => {
                    if let Some(dialog) = self.state.pair.as_mut() {
                        if dialog.printer_id == printer_id {
                            match result {
                                Ok(()) => self.state.pair = None,
                                Err(message) => {
                                    dialog.busy = false;
                                    dialog.error = Some(message.clone());
                                    self.backend.notice(NoticeLevel::Error, message);
                                }
                            }
                        }
                    }
                }
                ActionDone::Removed(saved_id, result) => {
                    if self.state.removing == Some(saved_id) {
                        self.state.removing = None;
                    }
                    if let Err(message) = result {
                        self.backend.notice(NoticeLevel::Error, message);
                    }
                }
                ActionDone::Tested(saved_id, result) => {
                    if self.state.testing == Some(saved_id) {
                        self.state.testing = None;
                    }
                    match result {
                        Ok(()) => self
                            .backend
                            .notice(NoticeLevel::Info, "Test page sent to the Windows queue"),
                        Err(message) => self.backend.notice(NoticeLevel::Error, message),
                    }
                }
                ActionDone::Firewall(result) => {
                    self.state.firewall_busy = false;
                    match result {
                        Ok(()) => self
                            .backend
                            .notice(NoticeLevel::Info, "Firewall rules installed"),
                        Err(message) => self.backend.notice(NoticeLevel::Error, message),
                    }
                }
                ActionDone::UpdateChecked(result) => self.finish_update_check(result),
            }
        }
    }

    fn finish_update_check(&mut self, result: Result<Option<update::Release>, String>) {
        self.state.updates.checking = false;
        let now = crate::config::now_unix();
        self.state.updates.last_check = now;
        let mut cfg = self.backend.config();
        cfg.updates.last_check = now;
        match result {
            Ok(available) => {
                self.state.updates.checked = true;
                self.state.updates.error = None;
                if let Some(release) = &available {
                    let already_notified = cfg.updates.last_notified_version.as_deref()
                        == Some(release.version.as_str());
                    if !already_notified {
                        cfg.updates.last_notified_version = Some(release.version.clone());
                        self.backend.notice(
                            NoticeLevel::Info,
                            format!(
                                "peerPrint v{} is available — see Settings → Updates",
                                release.version
                            ),
                        );
                    }
                }
                self.state.updates.available = available.clone();
                cfg.updates.available = available.map(|release| crate::config::CachedRelease {
                    version: release.version,
                    html_url: release.html_url,
                });
                cfg.updates.last_error = None;
            }
            Err(message) => {
                tracing::warn!("update check failed: {message}");
                self.state.updates.error = Some(message.clone());
                cfg.updates.last_error = Some(message);
            }
        }
        if let Err(err) = self.backend.set_config(cfg) {
            tracing::warn!("saving the update check time failed: {err:#}");
        }
    }

    /// Runs at most one automatic check per day, and only when enabled.
    fn maybe_check_updates(&mut self, ctx: &egui::Context) {
        if self.state.updates.checking {
            return;
        }
        let cfg = self.backend.config();
        self.state.updates.last_check = cfg.updates.last_check;
        if self.state.updates.available.is_none() {
            // A restart inside the daily window still shows the known release.
            self.state.updates.available = cfg
                .updates
                .available
                .as_ref()
                .and_then(|cached| update::cached(&cached.version, &cached.html_url));
        }
        if self.state.updates.error.is_none() && !self.state.updates.checked {
            // Preserve the last outcome instead of claiming the build is current.
            self.state.updates.error = cfg.updates.last_error.clone();
        }
        if !cfg.updates.auto_check {
            return;
        }
        if cfg.updates.last_check != 0 {
            let elapsed = crate::config::now_unix().saturating_sub(cfg.updates.last_check);
            if elapsed < update::CHECK_INTERVAL_SECS {
                return;
            }
        }
        self.start_update_check(ctx);
    }

    pub fn start_update_check(&mut self, ctx: &egui::Context) {
        if self.state.updates.checking {
            return;
        }
        self.state.updates.checking = true;
        self.state.updates.error = None;
        let sender = self.sender.clone();
        let ctx = ctx.clone();
        self.backend.runtime().spawn(async move {
            let result = match tokio::task::spawn_blocking(update::check).await {
                Ok(result) => result.map_err(|err| format!("{err:#}")),
                Err(err) => Err(format!("the update task failed: {err}")),
            };
            let _ = sender.send(ActionDone::UpdateChecked(result));
            ctx.request_repaint();
        });
    }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        ctx.request_repaint_after(Duration::from_secs(1));
        self.drain_actions();
        self.maybe_check_updates(ctx);
        self.ensure_tray(ctx);
        let core = self.backend.core();
        notifications::sync(self, ctx, &core);
        self.handle_shell(ctx, &core, frame);
        if self.start_hidden && !self.hide_done {
            self.hide_done = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }
        if self.state.quit_requested && !self.quitting {
            self.quitting = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let core = self.backend.core();
        let p = theme::palette(ui);
        let cfg = self.backend.config();
        match self.state.tab {
            Tab::Host if !cfg.role.hosts() => self.state.tab = Tab::Client,
            Tab::Client if !cfg.role.clients() => self.state.tab = Tab::Host,
            _ => {}
        }
        if self.window_theme != Some(p.dark) {
            crate::win::titlebar::apply(frame, &p);
            self.window_theme = Some(p.dark);
        }
        resize_edges(ui.ctx(), ui.max_rect());
        self.title_bar(ui, &p);

        egui::Panel::left("app_nav")
            .exact_size(208.0)
            .show_separator_line(false)
            .frame(
                egui::Frame::NONE.fill(p.sidebar).inner_margin(egui::Margin {
                    left: 16,
                    right: 16,
                    top: 18,
                    bottom: 14,
                }),
            )
            .show(ui, |ui| self.sidebar(ui, &core));

        egui::CentralPanel::default()
            .frame(
                egui::Frame::NONE.fill(p.bg).inner_margin(egui::Margin {
                    left: 28,
                    right: 28,
                    top: 22,
                    bottom: 24,
                }),
            )
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        self.alerts(ui, &core);
                        match self.state.tab {
                            Tab::Host => host_view::show(self, ui),
                            Tab::Client => client_view::show(self, ui),
                            Tab::History => history_view::show(self, ui),
                            Tab::Settings => settings::show(self, ui, &core),
                        }
                    });
            });

        let ctx = ui.ctx().clone();
        notifications::draw_flyouts(self, &ctx);
        notifications::center(self, &ctx, &core);
    }
}

impl App {
    fn title_bar(&mut self, ui: &mut egui::Ui, p: &theme::Palette) {
        let ctx = ui.ctx().clone();
        let maximized = ctx.input(|input| input.viewport().maximized.unwrap_or(false));
        egui::Panel::top("app_titlebar")
            .exact_size(38.0)
            .show_separator_line(false)
            .frame(egui::Frame::NONE.fill(p.sidebar))
            .show(ui, |ui| {
                let rect = ui.max_rect();
                let buttons = 3.0 * 46.0;
                let drag_rect = egui::Rect::from_min_max(
                    rect.min,
                    egui::pos2(rect.right() - buttons, rect.bottom()),
                );
                let drag = ui.interact(
                    drag_rect,
                    egui::Id::new("titlebar_drag"),
                    egui::Sense::click_and_drag(),
                );
                if drag.double_clicked() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
                } else if drag.drag_started_by(egui::PointerButton::Primary) {
                    ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                }

                let mut next = egui::Rect::from_min_size(
                    egui::pos2(rect.right() - buttons, rect.top()),
                    egui::vec2(46.0, rect.height()),
                );
                let minimize = window_button(ui, next, '\u{E921}', p, false);
                next = next.translate(egui::vec2(46.0, 0.0));
                let toggle_max = window_button(
                    ui,
                    next,
                    if maximized { '\u{E923}' } else { '\u{E922}' },
                    p,
                    false,
                );
                next = next.translate(egui::vec2(46.0, 0.0));
                let close = window_button(ui, next, '\u{E8BB}', p, true);

                if minimize {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                }
                if toggle_max {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
                }
                if close {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
    }

    fn sidebar(&mut self, ui: &mut egui::Ui, core: &crate::service::CoreState) {
        let p = theme::palette(ui);
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            ui.add(
                egui::Image::new(egui::include_image!("../../assets/peerprint-256.png"))
                    .fit_to_exact_size(egui::vec2(28.0, 28.0)),
            );
            ui.add_space(4.0);
            ui.vertical(|ui| {
                ui.label(
                    egui::RichText::new("peerPrint")
                        .font(egui::FontId::new(16.0, theme::bold()))
                        .color(p.text),
                );
                ui.label(
                    egui::RichText::new(format!("Version {}", env!("CARGO_PKG_VERSION")))
                        .size(10.5)
                        .color(p.muted),
                );
            });
        });
        ui.add_space(26.0);
        ui.label(egui::RichText::new("WORKSPACE").size(9.5).color(p.muted));
        ui.add_space(2.0);
        let cfg = self.backend.config();
        if cfg.role.hosts() && self.nav_item(ui, Tab::Host, "Host") {
            self.state.tab = Tab::Host;
        }
        if cfg.role.clients() && self.nav_item(ui, Tab::Client, "Client") {
            self.state.tab = Tab::Client;
        }
        if self.nav_item(ui, Tab::History, "History") {
            self.state.tab = Tab::History;
        }
        if self.nav_item(ui, Tab::Settings, "Settings") {
            self.state.tab = Tab::Settings;
        }

        ui.add_space((ui.available_height() - 156.0).max(0.0));
        let hosting = cfg.host.enabled;
        let relays = core.relays.iter().filter(|relay| relay.listening).count();
        egui::Frame::new()
            .fill(p.card)
            .stroke(egui::Stroke::new(1.0, p.border))
            .corner_radius(11)
            .inner_margin(egui::Margin::symmetric(12, 10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let (color, text) = if !cfg.role.hosts() {
                        if self.backend.connected() {
                            (p.success, "Ready")
                        } else {
                            (p.muted, "Offline")
                        }
                    } else if core.host_running {
                        (p.success, "Hosting")
                    } else if core.host_error.is_some() {
                        (p.danger, "Host error")
                    } else {
                        (p.muted, "Host off")
                    };
                    theme::dot(ui, color);
                    ui.label(egui::RichText::new(text).size(12.0).color(p.text));
                });
                ui.add_space(2.0);
                let detail = if !cfg.role.hosts() {
                    format!(
                        "{} relay{} active",
                        relays,
                        if relays == 1 { "" } else { "s" }
                    )
                } else if hosting {
                    format!(
                        "{} relay{} active",
                        relays,
                        if relays == 1 { "" } else { "s" }
                    )
                } else {
                    "Sharing is off".to_string()
                };
                theme::muted(ui, &detail);
            });
        ui.add_space(8.0);
        theme::muted(ui, "Runs in the notification area.");
        ui.add_space(10.0);
        let unread = notifications::unread(core, self.state.seen_notice);
        let response = notifications::button(ui, unread, self.state.notices_open);
        self.state.bell_rect = response.rect;
        if response.clicked() {
            self.state.notices_open = !self.state.notices_open;
            if self.state.notices_open {
                self.state.seen_notice = core
                    .notices
                    .front()
                    .map(|notice| notice.id)
                    .unwrap_or(self.state.seen_notice);
                self.state.flyouts.clear();
            }
        }
    }

    fn nav_item(&self, ui: &mut egui::Ui, tab: Tab, label: &str) -> bool {
        use egui::{Align2, Color32, CornerRadius, CursorIcon, FontFamily, FontId, Sense, Vec2};
        let p = theme::palette(ui);
        let selected = self.state.tab == tab;
        let (rect, response) = ui.allocate_exact_size(
            Vec2::new(ui.available_width(), 38.0),
            Sense::click(),
        );
        let response = response.on_hover_cursor(CursorIcon::PointingHand);
        let hover = ui.ctx().animate_bool(response.id, response.hovered());
        if selected {
            ui.painter()
                .rect_filled(rect, CornerRadius::same(19), p.accent_soft);
            ui.painter()
                .circle_filled(egui::pos2(rect.left() + 9.0, rect.center().y), 2.5, p.accent);
        } else if hover > 0.0 {
            ui.painter().rect_filled(
                rect,
                CornerRadius::same(19),
                Color32::TRANSPARENT.lerp_to_gamma(p.hover, hover),
            );
        }
        ui.painter().text(
            egui::pos2(rect.left() + 22.0, rect.center().y),
            Align2::LEFT_CENTER,
            label,
            FontId::new(
                13.5,
                if selected {
                    theme::bold()
                } else {
                    FontFamily::Proportional
                },
            ),
            if selected { p.accent_text } else { p.secondary },
        );
        response.clicked()
    }
}

impl App {
    fn ensure_tray(&mut self, ctx: &egui::Context) {
        if self.tray.is_some() || self.tray_attempted {
            return;
        }
        self.tray_attempted = true;
        let cfg = self.backend.config();
        let hosting = cfg.host.enabled;
        let can_host = cfg.role.hosts();
        match tray::Tray::create(ctx.clone(), hosting, can_host, "peerPrint") {
            Some((tray, receiver)) => {
                self.tray = Some(tray);
                self.tray_rx = Some(receiver);
            }
            None => tracing::warn!("notification-area icon unavailable"),
        }
    }

    fn handle_shell(
        &mut self,
        ctx: &egui::Context,
        core: &crate::service::CoreState,
        frame: &eframe::Frame,
    ) {
        let close_requested = ctx.input(|input| input.viewport().close_requested());
        if !self.quitting && close_requested && !self.close_handled {
            self.close_handled = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            self.backend.notice(
                NoticeLevel::Info,
                "peerPrint keeps running in the notification area. Use Quit there to stop it.",
            );
            return;
        }
        if !close_requested {
            self.close_handled = false;
        }

        let mut actions = Vec::new();
        if crate::ipc::take_quit() && !self.quitting {
            self.quitting = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        if let Some(receiver) = &self.tray_rx {
            while let Ok(action) = receiver.try_recv() {
                actions.push(action);
            }
        }
        if crate::ipc::take_activation() {
            actions.push(tray::TrayAction::Open);
        }
        for action in actions {
            match action {
                tray::TrayAction::Open => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                    restore_window(frame);
                }
                tray::TrayAction::ToggleHosting => {
                    let mut cfg = self.backend.config();
                    if cfg.role.hosts() {
                        cfg.host.enabled = !cfg.host.enabled;
                        let enabled = cfg.host.enabled;
                        match self.backend.set_config(cfg) {
                            Ok(()) => self.backend.notice(
                                NoticeLevel::Info,
                                if enabled {
                                    "Sharing printers"
                                } else {
                                    "Stopped sharing printers"
                                },
                            ),
                            Err(err) => self.backend.notice(
                                NoticeLevel::Error,
                                format!("saving hosting state: {err:#}"),
                            ),
                        }
                    }
                }
                tray::TrayAction::Quit => {
                    self.quitting = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }

        let active_relays = core.relays.iter().filter(|relay| relay.listening).count();
        let hosting = self.backend.config().host.enabled;
        let status = match (hosting, core.host_running) {
            (true, true) => format!("peerPrint — sharing · {active_relays} relay(s) active"),
            (true, false) => "peerPrint — sharing on (no printers selected)".to_string(),
            (false, _) => format!("peerPrint — {active_relays} relay(s) active"),
        };
        if let Some(tray) = &mut self.tray {
            tray.set_hosting(hosting);
            tray.set_tooltip(&status);
        }
    }

    fn alerts(&self, ui: &mut egui::Ui, core: &crate::service::CoreState) {
        let mut shown = false;
        if !self.backend.connected() {
            theme::notice(
                ui,
                NoticeLevel::Warn,
                "Waiting for the peerPrint background service… If this stays, reinstall \
                 peerPrint to repair the Windows service.",
            );
            if let Some(error) = self.backend.last_error() {
                tracing::debug!("engine connection: {error}");
            }
            ui.add_space(6.0);
            shown = true;
        }
        if let Some(error) = &core.host_error {
            theme::notice(ui, NoticeLevel::Error, error);
            ui.add_space(6.0);
            shown = true;
        }
        if shown {
            ui.add_space(4.0);
        }
    }
}

fn restore_window(frame: &eframe::Frame) {
    let Ok(handle) = frame.window_handle() else {
        return;
    };
    let RawWindowHandle::Win32(win32) = handle.as_raw() else {
        return;
    };
    let hwnd = HWND(win32.hwnd.get() as *mut _);
    unsafe {
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        let _ = SetForegroundWindow(hwnd);
    }
}

pub fn format_age(unix: i64) -> String {
    let age = crate::config::now_unix().saturating_sub(unix);
    if age < 60 {
        "just now".to_string()
    } else if age < 3600 {
        format!("{}m ago", age / 60)
    } else if age < 86_400 {
        format!("{}h ago", age / 3600)
    } else {
        format!("{}d ago", age / 86_400)
    }
}

pub fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    let value = bytes as f64;
    if value < KB {
        format!("{bytes} B")
    } else if value < KB * KB {
        format!("{:.1} KB", value / KB)
    } else if value < KB * KB * KB {
        format!("{:.1} MB", value / (KB * KB))
    } else {
        format!("{:.2} GB", value / (KB * KB * KB))
    }
}

fn window_button(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    glyph: char,
    p: &theme::Palette,
    close: bool,
) -> bool {
    let response = ui.interact(
        rect,
        egui::Id::new(("window_button", glyph)),
        egui::Sense::click(),
    );
    let hovered = response.hovered();
    let background = if close && hovered {
        egui::Color32::from_rgb(196, 43, 28)
    } else if hovered {
        p.border
    } else {
        egui::Color32::TRANSPARENT
    };
    if background != egui::Color32::TRANSPARENT {
        ui.painter()
            .rect_filled(rect, egui::CornerRadius::ZERO, background);
    }
    let color = if close && hovered {
        egui::Color32::WHITE
    } else {
        p.secondary
    };
    if let Some(family) = theme::icon_font() {
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            glyph,
            egui::FontId::new(10.0, family),
            color,
        );
    }
    response.clicked()
}

/// The borderless window's client area covers the frame, so native edge
/// resizing is gone; reintroduce it from pointer position.
fn resize_edges(ctx: &egui::Context, rect: egui::Rect) {
    const EDGE: f32 = 6.0;
    if ctx.input(|input| input.viewport().maximized.unwrap_or(false)) {
        return;
    }
    let Some(pos) = ctx.input(|input| input.pointer.hover_pos()) else {
        return;
    };
    if !rect.contains(pos) {
        return;
    }
    let left = pos.x - rect.left() <= EDGE;
    let right = rect.right() - pos.x <= EDGE;
    let top = pos.y - rect.top() <= EDGE;
    let bottom = rect.bottom() - pos.y <= EDGE;
    use egui::viewport::ResizeDirection;
    let direction = match (left, right, top, bottom) {
        (true, _, true, _) => Some(ResizeDirection::NorthWest),
        (_, true, true, _) => Some(ResizeDirection::NorthEast),
        (true, _, _, true) => Some(ResizeDirection::SouthWest),
        (_, true, _, true) => Some(ResizeDirection::SouthEast),
        (true, _, _, _) => Some(ResizeDirection::West),
        (_, true, _, _) => Some(ResizeDirection::East),
        (_, _, true, _) => Some(ResizeDirection::North),
        (_, _, _, true) => Some(ResizeDirection::South),
        _ => None,
    };
    let Some(direction) = direction else {
        return;
    };
    ctx.set_cursor_icon(match direction {
        ResizeDirection::North | ResizeDirection::South => egui::CursorIcon::ResizeVertical,
        ResizeDirection::East | ResizeDirection::West => egui::CursorIcon::ResizeHorizontal,
        ResizeDirection::NorthWest | ResizeDirection::SouthEast => egui::CursorIcon::ResizeNwSe,
        ResizeDirection::NorthEast | ResizeDirection::SouthWest => egui::CursorIcon::ResizeNeSw,
    });
    if ctx.input(|input| input.pointer.primary_pressed()) {
        ctx.send_viewport_cmd(egui::ViewportCommand::BeginResize(direction));
    }
}
