//! Default speaker volume/mute and microphone mute through IAudioEndpointVolume.
//! The endpoints stay open and their change callbacks post to the menu window.

use std::cell::RefCell;

use windows::core::{implement, Result, PCWSTR};
use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::Media::Audio::Endpoints::{IAudioEndpointVolume, IAudioEndpointVolumeCallback, IAudioEndpointVolumeCallback_Impl};
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};
use windows_sys::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

use crate::{state, util::log};

/// A volume or mute changed on a watched endpoint.
pub const WM_AUDIO_CHANGED: u32 = WM_APP + 3;
/// A default device changed; the endpoints have to be opened again.
pub const WM_AUDIO_DEVICES: u32 = WM_APP + 4;

const VOLUME_STEP: f32 = 0.02;

pub struct Output {
    pub level: f32,
    pub muted: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mic {
    Missing,
    /// There is a mic but its state couldn't be read.
    Unknown,
    Live,
    Muted,
}

/// The volume control of a default device, optionally reporting its changes.
struct Endpoint {
    volume: IAudioEndpointVolume,
    watcher: Option<IAudioEndpointVolumeCallback>,
}

impl Endpoint {
    fn default_device(enumerator: &IMMDeviceEnumerator, flow: EDataFlow, role: ERole, watch: bool) -> Option<Self> {
        // fails with E_NOTFOUND when there is no such device: not worth logging
        let device = unsafe { enumerator.GetDefaultAudioEndpoint(flow, role) }.ok()?;
        let volume: IAudioEndpointVolume = unsafe { device.Activate(CLSCTX_ALL, None) }
            .inspect_err(|e| log(&format!("audio: activating endpoint volume failed ({e})")))
            .ok()?;
        let watcher = watch.then(|| IAudioEndpointVolumeCallback::from(VolumeWatcher)).filter(|watcher| {
            unsafe { volume.RegisterControlChangeNotify(watcher) }
                .inspect_err(|e| log(&format!("audio: volume notifications unavailable ({e})")))
                .is_ok()
        });
        Some(Endpoint { volume, watcher })
    }

    fn level(&self) -> Option<f32> {
        unsafe { self.volume.GetMasterVolumeLevelScalar() }.ok()
    }

    fn set_level(&self, level: f32) {
        if let Err(e) = unsafe { self.volume.SetMasterVolumeLevelScalar(level.clamp(0.0, 1.0), std::ptr::null()) } {
            log(&format!("audio: setting volume failed ({e})"));
        }
    }

    fn muted(&self) -> Option<bool> {
        unsafe { self.volume.GetMute() }.ok().map(|m| m.as_bool())
    }

    fn set_muted(&self, mute: bool) {
        if let Err(e) = unsafe { self.volume.SetMute(mute, std::ptr::null()) } {
            log(&format!("audio: setting mute failed ({e})"));
        }
    }
}

impl Drop for Endpoint {
    fn drop(&mut self) {
        if let Some(watcher) = &self.watcher {
            let _ = unsafe { self.volume.UnregisterControlChangeNotify(watcher) };
        }
    }
}

struct Audio {
    enumerator: IMMDeviceEnumerator,
    devices_watcher: Option<IMMNotificationClient>,
    speakers: Option<Endpoint>,
    mic: Option<Endpoint>,
    /// The mic calls and Discord use; muted together with `mic`.
    comms_mic: Option<Endpoint>,
}

impl Audio {
    fn open_endpoints(&mut self) {
        self.speakers = Endpoint::default_device(&self.enumerator, eRender, eMultimedia, true);
        self.mic = Endpoint::default_device(&self.enumerator, eCapture, eConsole, true);
        self.comms_mic = Endpoint::default_device(&self.enumerator, eCapture, eCommunications, false);
    }
}

impl Drop for Audio {
    fn drop(&mut self) {
        if let Some(watcher) = &self.devices_watcher {
            let _ = unsafe { self.enumerator.UnregisterEndpointNotificationCallback(watcher) };
        }
    }
}

thread_local! {
    static AUDIO: RefCell<Option<Audio>> = const { RefCell::new(None) };
}

fn with<R>(f: impl FnOnce(&mut Audio) -> Option<R>) -> Option<R> {
    AUDIO.with(|a| a.borrow_mut().as_mut().and_then(f))
}

/// Opens the default endpoints and starts watching them. Call on the UI thread.
pub fn init() {
    let enumerator: IMMDeviceEnumerator = match unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) } {
        Ok(e) => e,
        Err(e) => return log(&format!("audio: no device enumerator ({e})")),
    };
    let watcher = IMMNotificationClient::from(DevicesWatcher);
    let devices_watcher = unsafe { enumerator.RegisterEndpointNotificationCallback(&watcher) }
        .inspect_err(|e| log(&format!("audio: device notifications unavailable ({e})")))
        .ok()
        .map(|()| watcher);
    let mut audio = Audio { enumerator, devices_watcher, speakers: None, mic: None, comms_mic: None };
    audio.open_endpoints();
    AUDIO.with(|a| *a.borrow_mut() = Some(audio));
}

/// Releases everything before COM shuts down.
pub fn shut_down() {
    AUDIO.with(|a| a.borrow_mut().take());
}

/// Follows a change of default device.
pub fn reopen() {
    with(|a| {
        a.open_endpoints();
        Some(())
    });
}

pub fn output() -> Option<Output> {
    with(|a| {
        let e = a.speakers.as_ref()?;
        Some(Output { level: e.level()?, muted: e.muted()? })
    })
}

/// Moves the volume one step, snapped to the step grid; raising it also unmutes.
pub fn step_volume(up: bool) {
    with(|a| {
        let e = a.speakers.as_ref()?;
        let delta = if up { VOLUME_STEP } else { -VOLUME_STEP };
        e.set_level(((e.level()? + delta) / VOLUME_STEP).round() * VOLUME_STEP);
        if up {
            e.set_muted(false);
        }
        Some(())
    });
}

pub fn toggle_output_mute() {
    with(|a| {
        let e = a.speakers.as_ref()?;
        e.set_muted(!e.muted()?);
        Some(())
    });
}

pub fn mic() -> Mic {
    with(|a| {
        Some(match a.mic.as_ref().map(Endpoint::muted) {
            None => Mic::Missing,
            Some(None) => Mic::Unknown,
            Some(Some(true)) => Mic::Muted,
            Some(Some(false)) => Mic::Live,
        })
    })
    .unwrap_or(Mic::Missing)
}

/// Mutes or unmutes both the default mic and the default communications mic.
pub fn toggle_mic() {
    with(|a| {
        let mute = !a.mic.as_ref()?.muted()?;
        for e in [&a.mic, &a.comms_mic].into_iter().flatten() {
            e.set_muted(mute);
        }
        Some(())
    });
}

// The callbacks run on audio-service threads, so they only post to the UI thread.

#[implement(IAudioEndpointVolumeCallback)]
struct VolumeWatcher;

impl IAudioEndpointVolumeCallback_Impl for VolumeWatcher_Impl {
    fn OnNotify(&self, _: *mut AUDIO_VOLUME_NOTIFICATION_DATA) -> Result<()> {
        unsafe { PostMessageW(state::menu_window(), WM_AUDIO_CHANGED, 0, 0) };
        Ok(())
    }
}

#[implement(IMMNotificationClient)]
struct DevicesWatcher;

impl IMMNotificationClient_Impl for DevicesWatcher_Impl {
    fn OnDefaultDeviceChanged(&self, flow: EDataFlow, role: ERole, _: &PCWSTR) -> Result<()> {
        let watched = (flow == eRender && role == eMultimedia) || (flow == eCapture && (role == eConsole || role == eCommunications));
        if watched {
            unsafe { PostMessageW(state::menu_window(), WM_AUDIO_DEVICES, 0, 0) };
        }
        Ok(())
    }

    // a default endpoint that goes away also changes the default, so these need no handling
    fn OnDeviceStateChanged(&self, _: &PCWSTR, _: DEVICE_STATE) -> Result<()> {
        Ok(())
    }

    fn OnDeviceAdded(&self, _: &PCWSTR) -> Result<()> {
        Ok(())
    }

    fn OnDeviceRemoved(&self, _: &PCWSTR) -> Result<()> {
        Ok(())
    }

    fn OnPropertyValueChanged(&self, _: &PCWSTR, _: &PROPERTYKEY) -> Result<()> {
        Ok(())
    }
}
