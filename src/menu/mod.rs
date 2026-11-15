//! The Home-button menu: state, navigation and what each row does.

mod draw;

use std::cell::RefCell;
use std::mem::{size_of, zeroed};
use std::ptr::null;
use std::sync::atomic::Ordering::Relaxed;

use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
use windows_sys::Win32::Graphics::Dwm::DwmFlush;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{VK_MEDIA_NEXT_TRACK, VK_MEDIA_PLAY_PAUSE, VK_MEDIA_PREV_TRACK};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::pad::PadEvent;
use crate::{apps, audio, capture, input, keyboard, state, tray};

pub use draw::paint;

const TIMER_CLOSE_CHECK: usize = 2;
const TIMER_CONFIRM_TIMEOUT: usize = 3;
pub const TIMERS: [usize; 2] = [TIMER_CLOSE_CHECK, TIMER_CONFIRM_TIMEOUT];

const CLOSE_CHECK_MS: u32 = 1500;
const CONFIRM_TIMEOUT_MS: u32 = 5000;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Item {
    Mouse,
    Keyboard,
    VoiceAccess,
    Sound,
    Mic,
    SwitchApp,
    Screenshot,
    ShowDesktop,
    CloseApp,
    Power,
}

impl Item {
    pub const ALL: [Item; 10] = [
        Item::Mouse,
        Item::Keyboard,
        Item::VoiceAccess,
        Item::Sound,
        Item::Mic,
        Item::SwitchApp,
        Item::Screenshot,
        Item::ShowDesktop,
        Item::CloseApp,
        Item::Power,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Item::Mouse => "Mouse",
            Item::Keyboard => "On-screen keyboard",
            Item::VoiceAccess => "Voice Access",
            Item::Sound => "Sound",
            Item::Mic => "Mic",
            Item::SwitchApp => "Switch app",
            Item::Screenshot => "Screenshot",
            Item::ShowDesktop => "Show desktop",
            Item::CloseApp => "Close app",
            Item::Power => "Power",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SoundButton {
    Previous,
    PlayPause,
    Next,
    Mute,
    VolumeDown,
    VolumeUp,
}

impl SoundButton {
    pub const ALL: [SoundButton; 6] = [
        SoundButton::Previous,
        SoundButton::PlayPause,
        SoundButton::Next,
        SoundButton::Mute,
        SoundButton::VolumeDown,
        SoundButton::VolumeUp,
    ];

    fn repeats(self) -> bool {
        matches!(self, SoundButton::VolumeDown | SoundButton::VolumeUp)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PowerButton {
    Sleep,
    Restart,
    ShutDown,
    Exit,
}

impl PowerButton {
    pub const ALL: [PowerButton; 4] = [PowerButton::Sleep, PowerButton::Restart, PowerButton::ShutDown, PowerButton::Exit];

    pub fn label(self) -> &'static str {
        match self {
            PowerButton::Sleep => "Sleep",
            PowerButton::Restart => "Restart",
            PowerButton::ShutDown => "Shut down",
            PowerButton::Exit => "Exit menu",
        }
    }
}

/// Progress of Close app on the current target.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CloseState {
    Idle,
    Closing,
    /// Still open after the close request: A now force-closes.
    Refused,
    /// Still open, and force closing isn't safe for this app.
    Failed,
}

struct Fonts {
    scale: f32,
    big: HFONT,
    small: HFONT,
    icons: HFONT,
}

impl Fonts {
    fn for_scale(&mut self, scale: f32) {
        if self.scale == scale {
            return;
        }
        let make = |face: &str, px: f32, weight: i32| unsafe {
            CreateFontW(-((px * scale) as i32), 0, 0, 0, weight, 0, 0, 0, 0, 0, 0, CLEARTYPE_QUALITY as _, 0, crate::util::wide(face).as_ptr())
        };
        self.delete();
        self.big = make("Segoe UI", 17.0, 500);
        self.small = make("Segoe UI", 12.0, 600);
        self.icons = make("Segoe MDL2 Assets", 16.0, 400);
        self.scale = scale;
    }

    fn delete(&mut self) {
        for f in [self.big, self.small, self.icons] {
            if !f.is_null() {
                unsafe { DeleteObject(f) };
            }
        }
    }
}

impl Drop for Fonts {
    fn drop(&mut self) {
        self.delete();
    }
}

pub struct Menu {
    row: usize,
    sound: usize,
    power: usize,
    confirm: Option<PowerButton>,
    mouse_on: bool,
    keyboard_on: bool,
    voice_on: bool,
    mic: audio::Mic,
    output: Option<audio::Output>,
    target: Option<(HWND, String)>,
    close: CloseState,
    apps: Vec<(HWND, String)>,
    app: usize,
    /// Screen area of the monitor the menu opened on (what Full screen captures).
    monitor: RECT,
    /// Had focus before the menu opened; gets it back when the menu closes.
    previous: HWND,
    fonts: Fonts,
}

impl Menu {
    fn new() -> Self {
        Menu {
            row: 0,
            sound: SoundButton::PlayPause as usize,
            power: 0,
            confirm: None,
            mouse_on: false,
            keyboard_on: false,
            voice_on: false,
            mic: audio::Mic::Missing,
            output: None,
            target: None,
            close: CloseState::Idle,
            apps: Vec::new(),
            app: 0,
            monitor: RECT { left: 0, top: 0, right: 0, bottom: 0 },
            previous: std::ptr::null_mut(),
            fonts: Fonts { scale: 0.0, big: std::ptr::null_mut(), small: std::ptr::null_mut(), icons: std::ptr::null_mut() },
        }
    }

    /// Back to a freshly opened state; the fonts are kept.
    fn reset(&mut self) {
        let mut fresh = Menu::new();
        std::mem::swap(&mut fresh.fonts, &mut self.fonts);
        *self = fresh;
    }

    fn item(&self) -> Item {
        Item::ALL[self.row]
    }

    fn sound_button(&self) -> SoundButton {
        SoundButton::ALL[self.sound]
    }

    fn disabled(&self, item: Item) -> bool {
        match item {
            Item::CloseApp => self.target.is_none(),
            Item::SwitchApp => self.apps.is_empty(),
            _ => false,
        }
    }
}

thread_local! {
    static MENU: RefCell<Menu> = RefCell::new(Menu::new());
}

fn with<R>(f: impl FnOnce(&mut Menu) -> R) -> R {
    MENU.with(|m| f(&mut m.borrow_mut()))
}

fn repaint(h: HWND) {
    unsafe { InvalidateRect(h, null(), 0) };
}

pub fn handle(h: HWND, event: PadEvent) {
    if event == PadEvent::Home {
        return if state::menu_open() { close(h) } else { open_from_home(h) };
    }
    if !state::menu_open() {
        return;
    }
    match event {
        PadEvent::Back => {
            if with(|m| m.confirm.take()).is_some() {
                repaint(h);
            } else {
                close(h);
            }
        }
        PadEvent::Up | PadEvent::Down => move_row(h, event == PadEvent::Down),
        PadEvent::Left | PadEvent::Right => move_within_row(h, event == PadEvent::Right),
        PadEvent::Select => select(h),
        PadEvent::SelectRepeat => {
            let held = with(|m| (m.item() == Item::Sound).then(|| m.sound_button()));
            if let Some(button) = held.filter(|b| b.repeats()) {
                press_sound(button);
            }
        }
        PadEvent::VolumeDown | PadEvent::VolumeUp => audio::step_volume(event == PadEvent::VolumeUp),
        PadEvent::PrevTrack => input::tap(VK_MEDIA_PREV_TRACK),
        PadEvent::NextTrack => input::tap(VK_MEDIA_NEXT_TRACK),
        PadEvent::Home => {}
    }
}

pub fn on_timer(h: HWND, id: usize) {
    unsafe { KillTimer(h, id) };
    match id {
        TIMER_CLOSE_CHECK => check_close(h),
        TIMER_CONFIRM_TIMEOUT => {
            with(|m| m.confirm = None);
            repaint(h);
        }
        _ => {}
    }
}

/// Re-reads volume and mic state after an audio change notification.
pub fn refresh_audio(h: HWND) {
    with(|m| {
        m.output = audio::output();
        m.mic = audio::mic();
    });
    tray::update_mic_icon(h);
    if state::menu_open() {
        repaint(h);
    }
}

fn open_from_home(h: HWND) {
    // the touch keyboard takes over the controller while it's open
    let hide_keyboard = keyboard::touch_keyboard_visible();
    if hide_keyboard {
        keyboard::hide_touch_keyboard_async();
    }
    open(h);
    if hide_keyboard {
        with(|m| m.keyboard_on = false);
    }
}

/// The menu closes when something else takes focus.
pub fn on_deactivate(h: HWND) {
    if state::menu_open() {
        close(h);
    }
}

fn open(h: HWND) {
    let foreground = unsafe { GetForegroundWindow() };
    let monitor = unsafe {
        if foreground.is_null() {
            let mut pt: POINT = zeroed();
            GetCursorPos(&mut pt);
            MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST)
        } else {
            MonitorFromWindow(foreground, MONITOR_DEFAULTTONEAREST)
        }
    };
    let mut info: MONITORINFO = unsafe { zeroed() };
    info.cbSize = size_of::<MONITORINFO>() as u32;
    let (mut dpi, mut dpi_y) = (96u32, 96u32);
    unsafe {
        GetMonitorInfoW(monitor, &mut info);
        GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi, &mut dpi_y);
    }
    let scale = dpi as f32 / 96.0;

    let target = apps::close_target(foreground);
    let current = target.as_ref().map(|t| t.0);
    with(|m| {
        m.reset();
        m.mouse_on = state::mouse_mode();
        m.keyboard_on = keyboard::visible();
        m.voice_on = apps::voice_access_running();
        m.mic = audio::mic();
        m.output = audio::output();
        m.apps = apps::open_apps().into_iter().filter(|(w, _)| Some(*w) != current).collect();
        m.target = target;
        m.monitor = info.rcMonitor;
        m.previous = foreground;
        m.fonts.for_scale(scale);
    });

    let (w, ht) = draw::size(scale);
    let work = info.rcWork;
    let x = work.left + (work.right - work.left - w) / 2;
    let y = work.top + (work.bottom - work.top - ht) / 2;
    unsafe {
        // focused while open, so apps behind (Terminal, Store apps) stop reacting to the controller
        SetWindowPos(h, HWND_TOPMOST, x, y, w, ht, SWP_SHOWWINDOW);
    }
    apps::activate(h);
    state::MENU_OPEN.store(true, Relaxed);
    repaint(h);
}

fn close(h: HWND) {
    // cleared first: hiding and handing focus back deactivate the window, which calls back in here
    state::MENU_OPEN.store(false, Relaxed);
    unsafe {
        ShowWindow(h, SW_HIDE);
        KillTimer(h, TIMER_CLOSE_CHECK);
        KillTimer(h, TIMER_CONFIRM_TIMEOUT);
    }
    let previous = with(|m| m.previous);
    if unsafe { IsWindow(previous) } != 0 {
        apps::activate(previous);
    }
}

fn move_row(h: HWND, down: bool) {
    with(|m| {
        m.confirm = None;
        let n = Item::ALL.len();
        let mut row = m.row;
        for _ in 0..n {
            row = if down { (row + 1) % n } else { (row + n - 1) % n };
            if !m.disabled(Item::ALL[row]) {
                m.row = row;
                break;
            }
        }
    });
    repaint(h);
}

fn move_within_row(h: HWND, right: bool) {
    let step = |i: usize, last: usize| if right { (i + 1).min(last) } else { i.saturating_sub(1) };
    with(|m| match m.item() {
        Item::Sound => m.sound = step(m.sound, SoundButton::ALL.len() - 1),
        Item::Power => {
            m.power = step(m.power, PowerButton::ALL.len() - 1);
            m.confirm = None;
        }
        Item::SwitchApp if !m.apps.is_empty() => {
            let n = m.apps.len();
            m.app = if right { (m.app + 1) % n } else { (m.app + n - 1) % n };
        }
        _ => {}
    });
    repaint(h);
}

fn select(h: HWND) {
    let (item, disabled) = with(|m| (m.item(), m.disabled(m.item())));
    if disabled {
        return;
    }
    match item {
        Item::Sound => press_sound(with(|m| m.sound_button())),
        Item::Mic => audio::toggle_mic(),
        Item::CloseApp => close_app(h),
        Item::Power => press_power(h),
        Item::SwitchApp => {
            let app = with(|m| m.apps.get(m.app).map(|a| a.0));
            close(h);
            if let Some(w) = app {
                apps::activate(w);
            }
        }
        Item::Mouse => {
            close(h);
            state::MOUSE_MODE.fetch_xor(true, Relaxed);
        }
        Item::Keyboard => {
            close(h);
            keyboard::toggle_async();
        }
        Item::VoiceAccess => {
            close(h);
            apps::toggle_voice_access();
        }
        Item::Screenshot => {
            close(h);
            // the menu has no hide animation, so one composed frame is enough for it to be gone
            unsafe { DwmFlush() };
            save_screenshot(h);
        }
        Item::ShowDesktop => {
            close(h);
            apps::toggle_desktop();
        }
    }
}

fn save_screenshot(h: HWND) {
    match capture::save(with(|m| m.monitor)) {
        Ok(path) => {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            tray::notify(h, "Screenshot saved", &format!("{name}\nin Pictures\\Screenshots. Click to open it."), Some(path));
        }
        Err(why) => tray::notify(h, "Screenshot failed", &why, None),
    }
}

fn press_sound(button: SoundButton) {
    match button {
        SoundButton::Previous => input::tap(VK_MEDIA_PREV_TRACK),
        SoundButton::PlayPause => input::tap(VK_MEDIA_PLAY_PAUSE),
        SoundButton::Next => input::tap(VK_MEDIA_NEXT_TRACK),
        SoundButton::Mute => audio::toggle_output_mute(),
        SoundButton::VolumeDown => audio::step_volume(false),
        SoundButton::VolumeUp => audio::step_volume(true),
    }
}

/// Every power action asks "Are you sure?" first: the first A arms it, the second runs it.
fn press_power(h: HWND) {
    let (button, armed) = with(|m| (PowerButton::ALL[m.power], m.confirm));
    if armed != Some(button) {
        with(|m| m.confirm = Some(button));
        unsafe { SetTimer(h, TIMER_CONFIRM_TIMEOUT, CONFIRM_TIMEOUT_MS, None) };
        return repaint(h);
    }
    close(h);
    match button {
        PowerButton::Sleep => apps::sleep(),
        PowerButton::Restart => apps::restart(),
        PowerButton::ShutDown => apps::shut_down(),
        PowerButton::Exit => unsafe {
            DestroyWindow(h);
        },
    }
}

/// First A asks the app to close. If it's still open a moment later, A force-closes it.
fn close_app(h: HWND) {
    let Some((target, state)) = with(|m| m.target.as_ref().map(|t| (t.0, m.close))) else {
        return;
    };
    match state {
        CloseState::Refused => {
            close(h);
            apps::force_close(target);
        }
        CloseState::Idle => {
            apps::request_close(target);
            with(|m| m.close = CloseState::Closing);
            unsafe { SetTimer(h, TIMER_CLOSE_CHECK, CLOSE_CHECK_MS, None) };
            repaint(h);
        }
        CloseState::Closing | CloseState::Failed => {}
    }
}

fn check_close(h: HWND) {
    if !state::menu_open() {
        return;
    }
    let Some(target) = with(|m| m.target.as_ref().map(|t| t.0)) else {
        return;
    };
    if !apps::still_open(target) {
        return close(h);
    }
    if let Some(dialog) = apps::pending_dialog(target) {
        close(h);
        return apps::activate(dialog);
    }
    let next = if apps::is_shared_host(target) { CloseState::Failed } else { CloseState::Refused };
    with(|m| m.close = next);
    repaint(h);
}
