use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock};

use eframe::egui;
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayAction {
    Open,
    ToggleHosting,
    Quit,
}

type SharedSender = Arc<Mutex<Sender<TrayAction>>>;

static SENDER: OnceLock<SharedSender> = OnceLock::new();

pub struct Tray {
    icon: TrayIcon,
    hosting: CheckMenuItem,
    tooltip: String,
}

impl Tray {
    pub fn create(
        ctx: egui::Context,
        hosting: bool,
        can_host: bool,
        tooltip: &str,
    ) -> Option<(Tray, Receiver<TrayAction>)> {
        let icon = eframe::icon_data::from_png_bytes(include_bytes!(
            "../../assets/peerprint-256.png"
        ))
        .ok()?;
        let icon = tray_icon::Icon::from_rgba(icon.rgba, icon.width, icon.height).ok()?;

        let menu = Menu::new();
        let open = MenuItem::with_id("open", "Open peerPrint", true, None);
        let hosting_item =
            CheckMenuItem::with_id("hosting", "Share my printers", can_host, hosting, None);
        let quit = MenuItem::with_id("quit", "Quit peerPrint", true, None);
        let separator = PredefinedMenuItem::separator();
        menu.append_items(&[&open, &hosting_item, &separator, &quit])
            .ok()?;

        let (sender, receiver) = channel();
        let sender = Arc::new(Mutex::new(sender));
        let _ = SENDER.set(sender.clone());

        let menu_sender = sender.clone();
        let menu_ctx = ctx.clone();
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            let action = match event.id().0.as_str() {
                "open" => TrayAction::Open,
                "hosting" => TrayAction::ToggleHosting,
                "quit" => TrayAction::Quit,
                _ => return,
            };
            if let Ok(sender) = menu_sender.lock() {
                let _ = sender.send(action);
            }
            menu_ctx.request_repaint();
        }));

        let click_sender = sender;
        let click_ctx = ctx;
        TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
            if matches!(
                event,
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } | TrayIconEvent::DoubleClick {
                    button: MouseButton::Left,
                    ..
                }
            ) {
                if let Ok(sender) = click_sender.lock() {
                    let _ = sender.send(TrayAction::Open);
                }
                click_ctx.request_repaint();
            }
        }));

        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip(tooltip)
            .with_icon(icon)
            .with_menu_on_left_click(false)
            .build()
            .ok()?;

        Some((
            Tray {
                icon: tray,
                hosting: hosting_item,
                tooltip: tooltip.to_string(),
            },
            receiver,
        ))
    }

    pub fn set_hosting(&self, hosting: bool) {
        if self.hosting.is_checked() != hosting {
            self.hosting.set_checked(hosting);
        }
    }

    pub fn set_tooltip(&mut self, text: &str) {
        if self.tooltip != text {
            if self.icon.set_tooltip(Some(text)).is_ok() {
                self.tooltip = text.to_string();
            }
        }
    }
}
