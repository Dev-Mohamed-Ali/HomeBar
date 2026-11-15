//! Synthetic input. Only single keys are ever sent, never modifiers like Win or Shift,
//! so a lost key-up can't leave anything held down.

use std::mem::{size_of, zeroed};

use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;

pub fn mouse(flags: MOUSE_EVENT_FLAGS, dx: i32, dy: i32, data: i32) {
    unsafe {
        let mut input: INPUT = zeroed();
        input.r#type = INPUT_MOUSE;
        input.Anonymous.mi = MOUSEINPUT { dx, dy, mouseData: data as _, dwFlags: flags, time: 0, dwExtraInfo: 0 };
        SendInput(1, &input, size_of::<INPUT>() as i32);
    }
}

/// Presses and releases one key, with the scan code and extended flag a real keyboard sends.
pub fn tap(vk: VIRTUAL_KEY) {
    let scan = unsafe { MapVirtualKeyW(vk as u32, MAPVK_VK_TO_VSC_EX) };
    let extended = matches!(scan >> 8, 0xe0 | 0xe1);
    let key = |up: bool| unsafe {
        let mut input: INPUT = zeroed();
        input.r#type = INPUT_KEYBOARD;
        let mut flags = if up { KEYEVENTF_KEYUP } else { 0 };
        if extended {
            flags |= KEYEVENTF_EXTENDEDKEY;
        }
        input.Anonymous.ki = KEYBDINPUT { wVk: vk, wScan: (scan & 0xff) as u16, dwFlags: flags, time: 0, dwExtraInfo: 0 };
        input
    };
    let inputs = [key(false), key(true)];
    unsafe { SendInput(inputs.len() as u32, inputs.as_ptr(), size_of::<INPUT>() as i32) };
}
