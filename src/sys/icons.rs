//! Program icons: extracted from executables with Win32, turned into PNG for GPUI and cached.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use windows_sys::Win32::Graphics::Gdi::{
    BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, DeleteObject, GetDC, GetDIBits, GetObjectW, HBITMAP, ReleaseDC,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, HICON, ICONINFO, PrivateExtractIconsW};

pub const SIZE: u32 = 48;

/// RGBA pixels, top-down.
pub struct Pixels {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

fn bitmap_pixels(hbm: HBITMAP) -> Option<(u32, u32, Vec<u8>)> {
    unsafe {
        let mut bm: BITMAP = std::mem::zeroed();
        if GetObjectW(hbm as _, std::mem::size_of::<BITMAP>() as i32, (&mut bm as *mut BITMAP).cast()) == 0 {
            return None;
        }
        let (w, h) = (bm.bmWidth as u32, bm.bmHeight.unsigned_abs());
        let mut info: BITMAPINFO = std::mem::zeroed();
        info.bmiHeader = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w as i32,
            biHeight: -(h as i32), // top-down
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..std::mem::zeroed()
        };
        let mut buf = vec![0u8; (w * h * 4) as usize];
        let dc = GetDC(std::ptr::null_mut());
        let lines = GetDIBits(dc, hbm, 0, h, buf.as_mut_ptr().cast(), &mut info, DIB_RGB_COLORS);
        ReleaseDC(std::ptr::null_mut(), dc);
        (lines != 0).then_some((w, h, buf))
    }
}

/// HICON → RGBA. Icons without an alpha channel get it from their mask.
fn icon_pixels(icon: HICON) -> Option<Pixels> {
    unsafe {
        let mut ii: ICONINFO = std::mem::zeroed();
        if GetIconInfo(icon, &mut ii) == 0 {
            return None;
        }
        let color = if ii.hbmColor.is_null() { None } else { bitmap_pixels(ii.hbmColor) };
        let mask = if ii.hbmMask.is_null() { None } else { bitmap_pixels(ii.hbmMask) };
        if !ii.hbmColor.is_null() {
            DeleteObject(ii.hbmColor as _);
        }
        if !ii.hbmMask.is_null() {
            DeleteObject(ii.hbmMask as _);
        }
        let (w, h, mut bgra) = color?;
        let has_alpha = bgra.chunks_exact(4).any(|p| p[3] != 0);
        if !has_alpha {
            if let Some((mw, _, m)) = mask.filter(|(mw, mh, _)| *mw == w && *mh >= h) {
                for (i, p) in bgra.chunks_exact_mut(4).enumerate() {
                    let (x, y) = (i as u32 % w, i as u32 / w);
                    let mi = ((y * mw + x) * 4) as usize;
                    p[3] = if m.get(mi).copied().unwrap_or(0) == 0 { 255 } else { 0 };
                }
            } else {
                for p in bgra.chunks_exact_mut(4) {
                    p[3] = 255;
                }
            }
        }
        for p in bgra.chunks_exact_mut(4) {
            p.swap(0, 2); // BGRA → RGBA
        }
        Some(Pixels { width: w, height: h, rgba: bgra })
    }
}

/// The best icon of `path` at `SIZE` px (the file's own icon, or what the shell shows for it).
pub fn extract(path: &Path) -> Option<Pixels> {
    let w = super::wide_path(path);
    unsafe {
        let mut icon: HICON = std::ptr::null_mut();
        let mut id = 0u32;
        let n = PrivateExtractIconsW(w.as_ptr(), 0, SIZE as i32, SIZE as i32, &mut icon, &mut id, 1, 0);
        if n >= 1 && !icon.is_null() && n != u32::MAX {
            let px = icon_pixels(icon);
            DestroyIcon(icon);
            if px.is_some() {
                return px;
            }
        }
        // files without their own icon resource: ask the shell
        use windows_sys::Win32::UI::Shell::{SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON, SHGetFileInfoW};
        let mut sfi: SHFILEINFOW = std::mem::zeroed();
        let r = SHGetFileInfoW(w.as_ptr(), 0, &mut sfi, std::mem::size_of::<SHFILEINFOW>() as u32, SHGFI_ICON | SHGFI_LARGEICON);
        if r != 0 && !sfi.hIcon.is_null() {
            let px = icon_pixels(sfi.hIcon);
            DestroyIcon(sfi.hIcon);
            return px;
        }
    }
    None
}

// ------------------------------------------------------------------ minimal PNG encoder (stored deflate)

fn crc32(data: &[u8]) -> u32 {
    static TABLE: LazyLock<[u32; 256]> = LazyLock::new(|| {
        let mut t = [0u32; 256];
        for (i, e) in t.iter_mut().enumerate() {
            let mut c = i as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            }
            *e = c;
        }
        t
    });
    !data.iter().fold(!0u32, |c, &b| TABLE[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8))
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &x in data {
        a = (a + x as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

pub fn encode_png(px: &Pixels) -> Vec<u8> {
    let row = px.width as usize * 4;
    let mut raw = Vec::with_capacity((row + 1) * px.height as usize);
    for y in 0..px.height as usize {
        raw.push(0); // filter: none
        raw.extend_from_slice(&px.rgba[y * row..(y + 1) * row]);
    }
    let mut z = vec![0x78, 0x01];
    let chunks: Vec<&[u8]> = raw.chunks(65535).collect();
    for (i, c) in chunks.iter().enumerate() {
        z.push((i == chunks.len() - 1) as u8);
        let len = c.len() as u16;
        z.extend(len.to_le_bytes());
        z.extend((!len).to_le_bytes());
        z.extend_from_slice(c);
    }
    z.extend(adler32(&raw).to_be_bytes());

    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut chunk = |kind: &[u8; 4], data: &[u8]| {
        out.extend((data.len() as u32).to_be_bytes());
        let mut body = kind.to_vec();
        body.extend_from_slice(data);
        out.extend_from_slice(&body);
        out.extend(crc32(&body).to_be_bytes());
    };
    let mut ihdr = Vec::new();
    ihdr.extend(px.width.to_be_bytes());
    ihdr.extend(px.height.to_be_bytes());
    ihdr.extend([8, 6, 0, 0, 0]); // 8-bit RGBA
    chunk(b"IHDR", &ihdr);
    chunk(b"IDAT", &z);
    chunk(b"IEND", &[]);
    out
}

// ------------------------------------------------------------------ cache

/// PNG bytes per file; None when the file has no icon.
static CACHE: LazyLock<Mutex<HashMap<PathBuf, Option<Arc<gpui_kit::Image>>>>> = LazyLock::new(Default::default);

/// Cached icon, if it was already extracted.
pub fn cached(path: &Path) -> Option<Option<Arc<gpui_kit::Image>>> {
    CACHE.lock().ok()?.get(path).cloned()
}

/// Extracts (or returns the cached) icon. Call from a background thread.
pub fn load(path: &Path) -> Option<Arc<gpui_kit::Image>> {
    if let Some(c) = cached(path) {
        return c;
    }
    let img = extract(path).map(|px| Arc::new(gpui_kit::Image::from_bytes(gpui_kit::ImageFormat::Png, encode_png(&px))));
    if let Ok(mut c) = CACHE.lock() {
        c.insert(path.to_path_buf(), img.clone());
    }
    img
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_notepad_icon() {
        let px = extract(&crate::sys::cmd::system_tool("notepad.exe")).expect("icon");
        assert_eq!(px.rgba.len(), (px.width * px.height * 4) as usize);
        assert!(px.rgba.chunks_exact(4).any(|p| p[3] > 0), "has visible pixels");
        let png = encode_png(&px);
        assert_eq!(&png[1..4], b"PNG");
    }

    #[test]
    fn crc_and_adler_known_values() {
        assert_eq!(crc32(b"IEND"), 0xAE42_6082);
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
    }
}
