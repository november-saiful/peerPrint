use std::ffi::c_void;

use eframe::egui;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWA_BORDER_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE,
    DWMWINDOWATTRIBUTE,
};

use crate::ui::theme::Palette;

/// Tints the native frame so it blends with the app palette. The window is
/// undecorated and its title bar is drawn by the UI, so only the border and the
/// dark-mode hint remain here.
pub fn apply(frame: &eframe::Frame, palette: &Palette) {
    let Ok(handle) = frame.window_handle() else {
        return;
    };
    let RawWindowHandle::Win32(win32) = handle.as_raw() else {
        return;
    };
    let hwnd = HWND(win32.hwnd.get() as *mut _);
    let dark: i32 = if palette.dark { 1 } else { 0 };
    unsafe {
        set(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &dark);
        set(hwnd, DWMWA_BORDER_COLOR, &colorref(palette.border));
    }
}

fn colorref(color: egui::Color32) -> u32 {
    u32::from(color.b()) << 16 | u32::from(color.g()) << 8 | u32::from(color.r())
}

unsafe fn set<T>(hwnd: HWND, attribute: DWMWINDOWATTRIBUTE, value: &T) {
    let _ = DwmSetWindowAttribute(
        hwnd,
        attribute,
        value as *const T as *const c_void,
        std::mem::size_of::<T>() as u32,
    );
}
