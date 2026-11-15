use std::io::Write;
use std::ptr::null_mut;

use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MESSAGEBOX_STYLE};

pub const APP_NAME: &str = "HomeBar";

/// NUL-terminated UTF-16 for Win32 `PCWSTR` parameters.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// Text from a UTF-16 buffer filled by Win32, up to the first NUL or `len`.
pub fn from_wide(buf: &[u16], len: usize) -> String {
    let end = buf[..len.min(buf.len())].iter().position(|&c| c == 0).unwrap_or(len.min(buf.len()));
    String::from_utf16_lossy(&buf[..end])
}

/// Win32 COLORREF (0x00BBGGRR).
pub const fn rgb(r: u8, g: u8, b: u8) -> u32 {
    r as u32 | (g as u32) << 8 | (b as u32) << 16
}

pub fn message_box(text: &str, style: MESSAGEBOX_STYLE) -> i32 {
    unsafe { MessageBoxW(null_mut(), wide(text).as_ptr(), wide(APP_NAME).as_ptr(), style) }
}

/// Appends to %TEMP%\HomeBar.log; used for failures worth diagnosing later.
pub fn log(msg: &str) {
    let path = std::env::temp_dir().join("HomeBar.log");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let _ = writeln!(f, "{secs} {msg}");
    }
}
