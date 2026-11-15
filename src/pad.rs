//! Controller polling on its own thread: menu navigation events and mouse mode.

use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::thread;
use std::time::{Duration, Instant};

use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
use windows_sys::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

use crate::{input, state, util::wide};

pub const WM_PAD: u32 = WM_APP + 1;

const DPAD_UP: u16 = 0x0001;
const DPAD_DOWN: u16 = 0x0002;
const DPAD_LEFT: u16 = 0x0004;
const DPAD_RIGHT: u16 = 0x0008;
const LB: u16 = 0x0100;
const RB: u16 = 0x0200;
const GUIDE: u16 = 0x0400;
const A: u16 = 0x1000;
const B: u16 = 0x2000;

const STICK_NAV: i16 = 16000;
const TRIGGER_PRESSED: u8 = 100;
const LEFT_DEADZONE: f32 = 7849.0;
const RIGHT_DEADZONE: f32 = 8689.0;
const CURSOR_SPEED: f32 = 1800.0; // px/s at full tilt
const SCROLL_SPEED: f32 = 1500.0; // wheel units/s, 120 = one notch

const TICK: Duration = Duration::from_millis(8);
const IDLE_TICK: Duration = Duration::from_millis(250);
const RESCAN: Duration = Duration::from_secs(1);
const REPEAT_DELAY: Duration = Duration::from_millis(350);
const REPEAT_RATE: Duration = Duration::from_millis(100);

/// Sent to the menu window as `WM_PAD`'s wParam.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(usize)]
pub enum PadEvent {
    Home = 1,
    Up,
    Down,
    Left,
    Right,
    Select,
    SelectRepeat,
    Back,
    VolumeDown,
    VolumeUp,
    PrevTrack,
    NextTrack,
}

impl PadEvent {
    const ALL: [PadEvent; 12] = [
        PadEvent::Home,
        PadEvent::Up,
        PadEvent::Down,
        PadEvent::Left,
        PadEvent::Right,
        PadEvent::Select,
        PadEvent::SelectRepeat,
        PadEvent::Back,
        PadEvent::VolumeDown,
        PadEvent::VolumeUp,
        PadEvent::PrevTrack,
        PadEvent::NextTrack,
    ];

    pub fn from_wparam(w: usize) -> Option<Self> {
        Self::ALL.into_iter().find(|&e| e as usize == w)
    }

    fn post(self) {
        unsafe { PostMessageW(state::menu_window(), WM_PAD, self as usize, 0) };
    }
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct GamepadState {
    packet: u32,
    buttons: u16,
    left_trigger: u8,
    right_trigger: u8,
    lx: i16,
    ly: i16,
    rx: i16,
    ry: i16,
}

type GetState = unsafe extern "system" fn(u32, *mut GamepadState) -> u32;

/// XInputGetStateEx (ordinal 100): the only XInput call that reports the Guide button.
pub struct XInput(GetState);

impl XInput {
    pub fn load() -> Option<Self> {
        unsafe {
            let dll = LoadLibraryW(wide("xinput1_4.dll").as_ptr());
            if dll.is_null() {
                return None;
            }
            let f = GetProcAddress(dll, 100usize as *const u8)?;
            Some(XInput(std::mem::transmute::<unsafe extern "system" fn() -> isize, GetState>(f)))
        }
    }

    fn read(&self, user: u32) -> Option<GamepadState> {
        let mut s = GamepadState::default();
        (unsafe { (self.0)(user, &mut s) } == 0).then_some(s)
    }
}

/// Fires `neg`/`pos` when a direction starts, then repeats while it's held.
struct Repeater {
    dir: i32,
    next: Instant,
}

impl Repeater {
    fn new() -> Self {
        Repeater { dir: 0, next: Instant::now() }
    }

    fn update(&mut self, dir: i32, neg: PadEvent, pos: PadEvent) {
        let now = Instant::now();
        let fire = if dir != self.dir {
            self.dir = dir;
            self.next = now + REPEAT_DELAY;
            dir != 0
        } else if dir != 0 && now >= self.next {
            self.next = now + REPEAT_RATE;
            true
        } else {
            false
        };
        if fire {
            if dir < 0 { neg } else { pos }.post();
        }
    }
}

fn axis(neg: bool, pos: bool) -> i32 {
    match (neg, pos) {
        (true, false) => -1,
        (false, true) => 1,
        _ => 0,
    }
}

/// Stick vector past the deadzone, magnitude eased to 0..=1.
fn stick(x: i16, y: i16, deadzone: f32) -> (f32, f32) {
    let (fx, fy) = (x as f32, y as f32);
    let m = (fx * fx + fy * fy).sqrt();
    if m <= deadzone {
        return (0.0, 0.0);
    }
    let n = ((m - deadzone) / (32767.0 - deadzone)).min(1.0);
    let s = n * n / m;
    (fx * s, fy * s)
}

/// Sub-pixel accumulator so slow stick movement still adds up.
#[derive(Default)]
struct Accum(f32);

impl Accum {
    fn take(&mut self, delta: f32) -> i32 {
        self.0 += delta;
        let whole = self.0.trunc();
        self.0 -= whole;
        whole as i32
    }
}

#[derive(Default)]
struct Mouse {
    x: Accum,
    y: Accum,
    wheel: Accum,
    hwheel: Accum,
    left: bool,
    right: bool,
}

impl Mouse {
    fn update(&mut self, s: &GamepadState, held: u16, dt: f32) {
        let (mx, my) = stick(s.lx, s.ly, LEFT_DEADZONE);
        let (dx, dy) = (self.x.take(mx * CURSOR_SPEED * dt), self.y.take(-my * CURSOR_SPEED * dt));
        if dx != 0 || dy != 0 {
            input::mouse(MOUSEEVENTF_MOVE, dx, dy, 0);
        }

        let (sx, sy) = stick(s.rx, s.ry, RIGHT_DEADZONE);
        let (v, h) = (self.wheel.take(sy * SCROLL_SPEED * dt), self.hwheel.take(sx * SCROLL_SPEED * dt));
        if v != 0 {
            input::mouse(MOUSEEVENTF_WHEEL, 0, 0, v);
        }
        if h != 0 {
            input::mouse(MOUSEEVENTF_HWHEEL, 0, 0, h);
        }

        self.set_left(held & A != 0);
        self.set_right(held & B != 0);
    }

    fn set_left(&mut self, down: bool) {
        if down != self.left {
            input::mouse(if down { MOUSEEVENTF_LEFTDOWN } else { MOUSEEVENTF_LEFTUP }, 0, 0, 0);
            self.left = down;
        }
    }

    fn set_right(&mut self, down: bool) {
        if down != self.right {
            input::mouse(if down { MOUSEEVENTF_RIGHTDOWN } else { MOUSEEVENTF_RIGHTUP }, 0, 0, 0);
            self.right = down;
        }
    }

    fn reset(&mut self) {
        self.set_left(false);
        self.set_right(false);
        *self = Mouse::default();
    }
}

/// Runs on the controller thread until `stop` is set; then releases any mouse button it holds.
pub fn run(xinput: XInput, stop: &AtomicBool) {
    let dt = TICK.as_secs_f32();
    let mut user: Option<u32> = None;
    let mut last_scan = Instant::now() - RESCAN;
    let mut prev = 0u16;
    // buttons that were already down when something else took them over; ignored until released
    let mut blocked = 0u16;
    let (mut vertical, mut horizontal, mut triggers) = (Repeater::new(), Repeater::new(), Repeater::new());
    let mut a_repeat_at = Instant::now();
    let mut mouse = Mouse::default();

    while !stop.load(Relaxed) {
        let Some(s) = user.and_then(|u| xinput.read(u)) else {
            user = None;
            mouse.reset();
            if last_scan.elapsed() >= RESCAN {
                last_scan = Instant::now();
                if let Some((u, s)) = (0..4).find_map(|u| xinput.read(u).map(|s| (u, s))) {
                    user = Some(u);
                    prev = s.buttons;
                    blocked = s.buttons;
                }
            }
            thread::sleep(if user.is_some() { TICK } else { IDLE_TICK });
            continue;
        };

        let down = s.buttons;
        let pressed = down & !prev;
        prev = down;
        blocked &= down;

        if pressed & GUIDE != 0 {
            PadEvent::Home.post();
            blocked |= down & (A | B);
        } else if state::menu_open() {
            mouse.reset();
            if pressed & A != 0 {
                PadEvent::Select.post();
                blocked |= A;
                a_repeat_at = Instant::now() + REPEAT_DELAY;
            } else if down & A != 0 && blocked & A == 0 && Instant::now() >= a_repeat_at {
                PadEvent::SelectRepeat.post();
                a_repeat_at = Instant::now() + REPEAT_RATE;
            } else if pressed & B != 0 {
                PadEvent::Back.post();
                blocked |= B;
            }
            if pressed & LB != 0 {
                PadEvent::PrevTrack.post();
            }
            if pressed & RB != 0 {
                PadEvent::NextTrack.post();
            }
            vertical.update(axis(down & DPAD_UP != 0 || s.ly > STICK_NAV, down & DPAD_DOWN != 0 || s.ly < -STICK_NAV), PadEvent::Up, PadEvent::Down);
            horizontal.update(axis(down & DPAD_LEFT != 0 || s.lx < -STICK_NAV, down & DPAD_RIGHT != 0 || s.lx > STICK_NAV), PadEvent::Left, PadEvent::Right);
            triggers.update(axis(s.left_trigger > TRIGGER_PRESSED, s.right_trigger > TRIGGER_PRESSED), PadEvent::VolumeDown, PadEvent::VolumeUp);
        } else {
            (vertical, horizontal, triggers) = (Repeater::new(), Repeater::new(), Repeater::new());
            if state::mouse_mode() {
                mouse.update(&s, down & !blocked, dt);
            } else {
                mouse.reset();
            }
        }
        thread::sleep(TICK);
    }
    mouse.reset();
}
