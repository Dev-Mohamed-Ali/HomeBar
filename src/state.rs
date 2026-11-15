//! State shared between the controller thread and the UI thread.

use std::ffi::c_void;
use std::ptr::null_mut;
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering::Relaxed};

use windows_sys::Win32::Foundation::HWND;

static MENU_WINDOW: AtomicPtr<c_void> = AtomicPtr::new(null_mut());
pub static MENU_OPEN: AtomicBool = AtomicBool::new(false);
pub static MOUSE_MODE: AtomicBool = AtomicBool::new(false);

pub fn set_menu_window(h: HWND) {
    MENU_WINDOW.store(h, Relaxed);
}

pub fn menu_window() -> HWND {
    MENU_WINDOW.load(Relaxed)
}

pub fn menu_open() -> bool {
    MENU_OPEN.load(Relaxed)
}

pub fn mouse_mode() -> bool {
    MOUSE_MODE.load(Relaxed)
}
