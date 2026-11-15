//! "Check for updates": compares this version with the latest GitHub release.

use std::ffi::c_void;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::thread;

use windows::core::HSTRING;
use windows::Data::Json::JsonObject;
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};
use windows_sys::Win32::Foundation::LPARAM;
use windows_sys::Win32::Networking::WinHttp::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::{apps, state, util::{message_box, wide}};

pub const REPO: &str = "Dev-Mohamed-Ali/HomeBar";
/// lParam is a `Box<String>` URL; handled by `open_posted_url`.
pub const WM_OPEN_URL: u32 = WM_APP + 5;
const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Per step (name lookup, connect, send, receive), so a dead network gives up instead of hanging.
const TIMEOUT_MS: i32 = 15_000;

static RUNNING: AtomicBool = AtomicBool::new(false);

/// One check at a time. The download page opens on the UI thread.
pub fn check_in_background() {
    if RUNNING.swap(true, Relaxed) {
        return;
    }
    thread::spawn(|| {
        // the release JSON is parsed with WinRT, which needs COM on this thread
        let com = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
        if let Some(url) = check(REPO) {
            let url = Box::into_raw(Box::new(url));
            if unsafe { PostMessageW(state::menu_window(), WM_OPEN_URL, 0, url as LPARAM) } == 0 {
                drop(unsafe { Box::from_raw(url) });
            }
        }
        if com {
            unsafe { CoUninitialize() };
        }
        RUNNING.store(false, Relaxed);
    });
}

pub fn open_posted_url(l: LPARAM) {
    let url = unsafe { Box::from_raw(l as *mut String) };
    apps::open(&url);
}

/// Shows the result in a message box; returns the download page if the user wants it opened.
pub fn check(repo: &str) -> Option<String> {
    let info = |text: &str| message_box(text, MB_OK | MB_ICONINFORMATION | MB_TOPMOST | MB_SETFOREGROUND);
    match get_github(&format!("/repos/{repo}/releases/latest")) {
        None => {
            info("Couldn't reach GitHub. Check your internet connection and try again.");
        }
        Some((404, _)) => {
            info(&format!("You have version {VERSION}.\n\nNo releases are published yet."));
        }
        Some((200, body)) => {
            let Some(release) = Release::parse(&body) else {
                info("GitHub sent a release this version of HomeBar can't read. Check the releases page instead.");
                return None;
            };
            let tag = release.tag;
            if parse_version(&tag) <= parse_version(VERSION) {
                info(&format!("You have the latest version ({VERSION})."));
                return None;
            }
            let ask = format!("Version {} is available (you have {VERSION}).\n\nOpen the download page?", tag.trim_start_matches(['v', 'V']));
            if message_box(&ask, MB_YESNO | MB_ICONINFORMATION | MB_TOPMOST | MB_SETFOREGROUND) == IDYES {
                return Some(release.page);
            }
        }
        Some((status, _)) => {
            info(&format!("GitHub answered with an error ({status}). Try again later."));
        }
    }
    None
}

/// Trailing zeros dropped, so "v1.2" and "1.2.0" compare equal.
fn parse_version(v: &str) -> Vec<u32> {
    let mut parts: Vec<u32> = v.trim_start_matches(['v', 'V']).split('.').map(|p| p.trim().parse().unwrap_or(0)).collect();
    while parts.last() == Some(&0) {
        parts.pop();
    }
    parts
}

/// The fields used from GitHub's "latest release" JSON.
struct Release {
    tag: String,
    page: String,
}

impl Release {
    fn parse(body: &str) -> Option<Self> {
        let json = JsonObject::Parse(&HSTRING::from(body)).ok()?;
        let field = |name: &str| json.GetNamedString(&HSTRING::from(name)).ok().map(|v| v.to_string_lossy());
        Some(Release { tag: field("tag_name")?, page: field("html_url")? })
    }
}

/// Closes WinHTTP handles on every exit path.
struct Handle(*mut c_void);

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { WinHttpCloseHandle(self.0) };
        }
    }
}

/// HTTPS GET on api.github.com; (status, body).
fn get_github(path: &str) -> Option<(u32, String)> {
    unsafe {
        let session = Handle(WinHttpOpen(wide("HomeBar").as_ptr(), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, null(), null(), 0));
        if session.0.is_null() || WinHttpSetTimeouts(session.0, TIMEOUT_MS, TIMEOUT_MS, TIMEOUT_MS, TIMEOUT_MS) == 0 {
            return None;
        }
        let connection = Handle(WinHttpConnect(session.0, wide("api.github.com").as_ptr(), 443, 0));
        if connection.0.is_null() {
            return None;
        }
        let request = Handle(WinHttpOpenRequest(connection.0, wide("GET").as_ptr(), wide(path).as_ptr(), null(), null(), null(), WINHTTP_FLAG_SECURE));
        if request.0.is_null()
            || WinHttpSendRequest(request.0, null(), 0, null(), 0, 0, 0) == 0
            || WinHttpReceiveResponse(request.0, null_mut()) == 0
        {
            return None;
        }

        let mut status = 0u32;
        let mut size = 4u32;
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            null(),
            &mut status as *mut u32 as *mut c_void,
            &mut size,
            null_mut(),
        );

        let mut body = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            let mut read = 0u32;
            if WinHttpReadData(request.0, chunk.as_mut_ptr() as *mut c_void, chunk.len() as u32, &mut read) == 0 {
                return None; // a cut-off body isn't a response
            }
            if read == 0 {
                break;
            }
            body.extend_from_slice(&chunk[..read as usize]);
        }
        Some((status, String::from_utf8_lossy(&body).into_owned()))
    }
}
