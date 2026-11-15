//! Full-screen capture of one monitor, saved as PNG through WIC.

use std::ffi::{c_void, OsString};
use std::mem::{size_of, zeroed};
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::ptr::null_mut;

use windows::core::{Result as ComResult, HSTRING};
use windows::Win32::Foundation::{E_POINTER, GENERIC_WRITE, WINCODEC_ERR_UNSUPPORTEDPIXELFORMAT};
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_INPROC_SERVER};
use windows::Win32::UI::Shell::{FOLDERID_Screenshots, SHGetKnownFolderPath, KF_FLAG_DEFAULT};
use windows_sys::Win32::Foundation::RECT;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::SystemInformation::GetLocalTime;

/// Captures `area` (screen coordinates) and saves it to the Screenshots folder.
pub fn save(area: RECT) -> Result<PathBuf, String> {
    let (width, height) = (area.right - area.left, area.bottom - area.top);
    if width <= 0 || height <= 0 {
        return Err("nothing to capture".into());
    }
    let pixels = grab(area, width, height).ok_or("couldn't read the screen")?;
    let path = screenshots_folder()?.join(file_name());
    if let Err(e) = write_png(&path, width as u32, height as u32, &pixels) {
        let _ = std::fs::remove_file(&path);
        return Err(format!("couldn't save the PNG ({e})"));
    }
    Ok(path)
}

/// Top-down BGRA pixels of the screen area, fully opaque.
fn grab(area: RECT, width: i32, height: i32) -> Option<Vec<u8>> {
    unsafe {
        let screen = GetDC(null_mut());
        let dc = CreateCompatibleDC(screen);
        let mut info: BITMAPINFO = zeroed();
        info.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
        info.bmiHeader.biWidth = width;
        info.bmiHeader.biHeight = -height;
        info.bmiHeader.biPlanes = 1;
        info.bmiHeader.biBitCount = 32;
        let mut bits: *mut c_void = null_mut();
        let dib = CreateDIBSection(screen, &info, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
        let mut result = None;
        if !dib.is_null() && !bits.is_null() {
            let old = SelectObject(dc, dib);
            if BitBlt(dc, 0, 0, width, height, screen, area.left, area.top, SRCCOPY | CAPTUREBLT) != 0 {
                GdiFlush();
                let mut pixels = std::slice::from_raw_parts(bits as *const u8, (width * height * 4) as usize).to_vec();
                for p in pixels.as_chunks_mut::<4>().0 {
                    p[3] = 0xff;
                }
                result = Some(pixels);
            }
            SelectObject(dc, old);
            DeleteObject(dib);
        }
        DeleteDC(dc);
        ReleaseDC(null_mut(), screen);
        result
    }
}

fn screenshots_folder() -> Result<PathBuf, String> {
    let folder = unsafe {
        let path = SHGetKnownFolderPath(&FOLDERID_Screenshots, KF_FLAG_DEFAULT, None).map_err(|e| format!("couldn't find the Screenshots folder ({e})"))?;
        let folder = PathBuf::from(OsString::from_wide(path.as_wide()));
        CoTaskMemFree(Some(path.0 as *const c_void));
        folder
    };
    std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
    Ok(folder)
}

fn file_name() -> String {
    let mut t = unsafe { zeroed() };
    unsafe { GetLocalTime(&mut t) };
    format!("HomeBar {:04}-{:02}-{:02} {:02}-{:02}-{:02}.png", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond)
}

/// Every COM object is released on return, so a failed file can be deleted afterwards.
fn write_png(path: &Path, width: u32, height: u32, pixels: &[u8]) -> ComResult<()> {
    unsafe {
        let factory: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
        let stream = factory.CreateStream()?;
        stream.InitializeFromFilename(&HSTRING::from(path), GENERIC_WRITE.0)?;
        let encoder = factory.CreateEncoder(&GUID_ContainerFormatPng, std::ptr::null())?;
        encoder.Initialize(&stream, WICBitmapEncoderNoCache)?;

        let (mut frame, mut options) = (None, None);
        encoder.CreateNewFrame(&mut frame, &mut options)?;
        let frame = frame.ok_or(E_POINTER)?;
        frame.Initialize(options.as_ref())?;
        frame.SetSize(width, height)?;
        let mut format = GUID_WICPixelFormat32bppBGRA;
        frame.SetPixelFormat(&mut format)?;
        if format != GUID_WICPixelFormat32bppBGRA {
            return Err(WINCODEC_ERR_UNSUPPORTEDPIXELFORMAT.into());
        }
        frame.WritePixels(height, width * 4, pixels)?;
        frame.Commit()?;
        encoder.Commit()
    }
}
