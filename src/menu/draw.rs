//! Owner-drawn GDI rendering of the menu, double-buffered.

use std::mem::zeroed;

use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::UI::WindowsAndMessaging::GetClientRect;

use super::{CloseState, Item, Menu, PowerButton, SoundButton, MENU};
use crate::audio::Mic;
use crate::util::{rgb, wide};

const BACKGROUND: u32 = rgb(0x1f, 0x1f, 0x1f);
const TEXT: u32 = rgb(0xf2, 0xf2, 0xf2);
const DIM: u32 = rgb(0x9a, 0x9a, 0x9a);
const ACCENT: u32 = rgb(0x00, 0x78, 0xd4);
const ON: u32 = rgb(0x6c, 0xcb, 0x5f);
const DANGER: u32 = rgb(0xd1, 0x34, 0x38);
const WARNING_TEXT: u32 = rgb(0xff, 0x8a, 0x8a);

// layout in 96-DPI pixels
const WIDTH: f32 = 440.0;
const HEADER: f32 = 56.0;
const ROW: f32 = 56.0;
const ROW_HEIGHT: f32 = 50.0;
const FOOTER: f32 = 44.0;
const SIDE: f32 = 12.0;
const TEXT_INSET: f32 = 18.0;
const VALUE_LEFT: f32 = 130.0;

/// Window size in physical pixels for a monitor scale factor.
pub fn size(scale: f32) -> (i32, i32) {
    let px = |v: f32| (v * scale) as i32;
    (px(WIDTH), px(HEADER) + Item::ALL.len() as i32 * px(ROW) + px(FOOTER))
}

struct Canvas {
    dc: HDC,
    scale: f32,
}

impl Canvas {
    fn px(&self, v: f32) -> i32 {
        (v * self.scale) as i32
    }

    fn text(&self, s: &str, r: RECT, color: u32, font: HFONT, align: DRAW_TEXT_FORMAT) {
        let mut r = r;
        unsafe {
            SelectObject(self.dc, font);
            SetTextColor(self.dc, color);
            DrawTextW(self.dc, wide(s).as_ptr(), -1, &mut r, align | DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | DT_END_ELLIPSIS);
        }
    }

    fn rounded(&self, r: RECT, color: u32, radius: f32) {
        unsafe {
            let brush = CreateSolidBrush(color);
            let old_brush = SelectObject(self.dc, brush);
            let old_pen = SelectObject(self.dc, GetStockObject(NULL_PEN));
            let radius = self.px(radius);
            RoundRect(self.dc, r.left, r.top, r.right, r.bottom, radius, radius);
            SelectObject(self.dc, old_brush);
            SelectObject(self.dc, old_pen);
            DeleteObject(brush);
        }
    }
}

pub fn paint(h: HWND) {
    unsafe {
        let mut ps: PAINTSTRUCT = zeroed();
        let dc = BeginPaint(h, &mut ps);
        let mut client: RECT = zeroed();
        GetClientRect(h, &mut client);
        let mem = CreateCompatibleDC(dc);
        let bitmap = CreateCompatibleBitmap(dc, client.right, client.bottom);
        let old_bitmap = SelectObject(mem, bitmap);
        let background = CreateSolidBrush(BACKGROUND);
        FillRect(mem, &client, background);
        DeleteObject(background);
        SetBkMode(mem, TRANSPARENT as _);
        let old_font = SelectObject(mem, GetStockObject(DEFAULT_GUI_FONT));

        MENU.with(|m| draw_menu(&m.borrow(), mem, client));

        BitBlt(dc, 0, 0, client.right, client.bottom, mem, 0, 0, SRCCOPY);
        SelectObject(mem, old_font);
        SelectObject(mem, old_bitmap);
        DeleteObject(bitmap);
        DeleteDC(mem);
        EndPaint(h, &ps);
    }
}

fn draw_menu(m: &Menu, dc: HDC, client: RECT) {
    let c = Canvas { dc, scale: m.fonts.scale };
    let header = RECT { left: c.px(24.0), top: c.px(14.0), right: client.right, bottom: c.px(50.0) };
    c.text("HOMEBAR", header, DIM, m.fonts.small, DT_LEFT);

    for (i, &item) in Item::ALL.iter().enumerate() {
        let top = c.px(HEADER) + i as i32 * c.px(ROW);
        let row = RECT { left: c.px(SIDE), top, right: client.right - c.px(SIDE), bottom: top + c.px(ROW_HEIGHT) };
        draw_row(m, &c, item, row, i == m.row);
    }

    let footer = RECT { left: 0, top: client.bottom - c.px(FOOTER), right: client.right, bottom: client.bottom - c.px(6.0) };
    c.text(footer_hint(m), footer, DIM, m.fonts.small, DT_CENTER);
}

fn footer_hint(m: &Menu) -> &'static str {
    if m.confirm.is_some() {
        return "Are you sure?      A  Yes      B  No";
    }
    match m.item() {
        Item::Sound | Item::Power => "\u{25C0} \u{25B6}  Choose      A  Press      B  Close",
        Item::SwitchApp => "\u{25C0} \u{25B6}  Choose app      A  Switch      B  Close",
        _ => "A  Select      B  Close",
    }
}

fn draw_row(m: &Menu, c: &Canvas, item: Item, row: RECT, selected: bool) {
    if selected {
        c.rounded(row, ACCENT, 12.0);
    }
    let label = RECT { left: row.left + c.px(TEXT_INSET), ..row };
    c.text(item.label(), label, if m.disabled(item) { DIM } else { TEXT }, m.fonts.big, DT_LEFT);

    let value = RECT { left: row.left + c.px(VALUE_LEFT), right: row.right - c.px(TEXT_INSET), ..row };
    let on_off = |on: bool| c.text(if on { "On" } else { "Off" }, value, if selected { TEXT } else if on { ON } else { DIM }, m.fonts.big, DT_RIGHT);
    let note = |s: &str, warn: bool| c.text(s, value, if selected { TEXT } else if warn { WARNING_TEXT } else { DIM }, m.fonts.big, DT_RIGHT);

    match item {
        Item::Mouse => on_off(m.mouse_on),
        Item::Keyboard => on_off(m.keyboard_on),
        Item::VoiceAccess => on_off(m.voice_on),
        Item::Mic => match m.mic {
            Mic::Live => on_off(true),
            Mic::Muted => on_off(false),
            Mic::Missing => note("no mic", false),
            Mic::Unknown => note("\u{2014}", false),
        },
        Item::SwitchApp => {
            if let Some((_, title)) = m.apps.get(m.app) {
                // arrows drawn on their own so a long title can't push them out
                let arrow = c.px(20.0);
                let color = if selected { TEXT } else { DIM };
                c.text("\u{25C0}", RECT { right: value.left + arrow, ..value }, color, m.fonts.small, DT_LEFT);
                c.text("\u{25B6}", RECT { left: value.right - arrow, ..value }, color, m.fonts.small, DT_RIGHT);
                c.text(title, RECT { left: value.left + arrow, right: value.right - arrow, ..value }, color, m.fonts.big, DT_CENTER);
            }
        }
        Item::CloseApp => match (m.close, &m.target) {
            (CloseState::Refused, _) => note("Still open \u{00B7} A = force close", true),
            (CloseState::Closing, _) => note("Closing\u{2026}", false),
            (CloseState::Failed, _) => note("Didn't close", false),
            (CloseState::Idle, Some((_, name))) => note(name, false),
            (CloseState::Idle, None) => {}
        },
        Item::Sound => draw_sound(m, c, row, selected),
        Item::Power => {
            let labels = PowerButton::ALL.map(PowerButton::label);
            let armed = m.confirm.and_then(|b| PowerButton::ALL.iter().position(|&p| p == b));
            draw_text_buttons(m, c, row, ButtonStrip { labels: &labels, width: 76.0, hot: selected.then_some(m.power), armed }, selected);
        }
        Item::Screenshot | Item::ShowDesktop => {}
    }
}

/// [prev] [play/pause] [next]  [mute]  [-] 45% [+], right-aligned.
fn draw_sound(m: &Menu, c: &Canvas, row: RECT, selected: bool) {
    let (button, gap, group, level_width) = (c.px(36.0), c.px(6.0), c.px(14.0), c.px(58.0));
    let (top, bottom) = (row.top + c.px(7.0), row.bottom - c.px(7.0));
    let mut right = row.right - c.px(10.0);
    let mut take = |width: i32, space_before: i32| {
        let r = RECT { left: right - width, top, right, bottom };
        right -= width + space_before;
        r
    };
    // right to left: + level -  mute  next play prev
    let up = take(button, 0);
    let level = take(level_width, 0);
    let down = take(button, group);
    let mute = take(button, group);
    let next = take(button, gap);
    let play = take(button, gap);
    let prev = take(button, 0);

    let muted = m.output.as_ref().is_some_and(|o| o.muted);
    let buttons = [
        (SoundButton::Previous, prev, "\u{E892}"),
        (SoundButton::PlayPause, play, "\u{E768}"),
        (SoundButton::Next, next, "\u{E893}"),
        (SoundButton::Mute, mute, if muted { "\u{E74F}" } else { "\u{E767}" }),
        (SoundButton::VolumeDown, down, "\u{E738}"),
        (SoundButton::VolumeUp, up, "\u{E710}"),
    ];
    for (b, r, glyph) in buttons {
        let hot = selected && m.sound_button() == b;
        if hot {
            c.rounded(r, TEXT, 8.0);
        }
        c.text(glyph, r, if hot { ACCENT } else if selected { TEXT } else { DIM }, m.fonts.icons, DT_CENTER);
    }

    let level_text = match &m.output {
        Some(o) if o.muted => "Muted".to_string(),
        Some(o) => format!("{}%", (o.level * 100.0).round() as i32),
        None => "\u{2014}".to_string(),
    };
    c.text(&level_text, level, if selected { TEXT } else { DIM }, m.fonts.big, DT_CENTER);
}

/// Right-aligned text buttons; `hot` is highlighted, `armed` (waiting for "Are you sure?") is red.
struct ButtonStrip<'a> {
    labels: &'a [&'a str],
    width: f32,
    hot: Option<usize>,
    armed: Option<usize>,
}

fn draw_text_buttons(m: &Menu, c: &Canvas, row: RECT, strip: ButtonStrip, selected: bool) {
    let ButtonStrip { labels, width, hot, armed } = strip;
    let (width, gap) = (c.px(width), c.px(6.0));
    let (top, bottom) = (row.top + c.px(8.0), row.bottom - c.px(8.0));
    let count = labels.len() as i32;
    for (i, label) in labels.iter().enumerate() {
        let right = row.right - c.px(10.0) - (count - 1 - i as i32) * (width + gap);
        let r = RECT { left: right - width, top, right, bottom };
        let (is_hot, is_armed) = (hot == Some(i), armed == Some(i));
        if is_armed {
            c.rounded(r, DANGER, 8.0);
        } else if is_hot {
            c.rounded(r, TEXT, 8.0);
        }
        let color = if is_armed { TEXT } else if is_hot { ACCENT } else if selected { TEXT } else { DIM };
        c.text(if is_armed { "Sure?" } else { label }, r, color, m.fonts.small, DT_CENTER);
    }
}