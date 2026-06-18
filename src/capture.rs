//! Native desktop screen capture (Windows GDI) for the on-demand `screenshot` command. Runs in the
//! interactive Session-1 tray (Session 0 has no display). Grabs the full virtual screen via BitBlt +
//! GetDIBits and encodes a PNG. (Blacks out on the UAC secure desktop / lock screen — a GDI limitation.)
#![cfg(windows)]

use anyhow::{anyhow, Result};
use std::mem::size_of;

use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits,
    ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HDC, HGDIOBJ,
    SRCCOPY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};

/// Capture the full virtual desktop and return PNG bytes.
pub fn capture_png() -> Result<Vec<u8>> {
    unsafe {
        let x = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let y = GetSystemMetrics(SM_YVIRTUALSCREEN);
        let w = GetSystemMetrics(SM_CXVIRTUALSCREEN);
        let h = GetSystemMetrics(SM_CYVIRTUALSCREEN);
        if w <= 0 || h <= 0 {
            return Err(anyhow!("invalid virtual screen size {w}x{h}"));
        }

        let screen = GetDC(None);
        if screen.is_invalid() {
            return Err(anyhow!("GetDC(screen) failed"));
        }
        let mem = CreateCompatibleDC(screen);
        let memdc = HDC(mem.0); // CreatedHDC -> HDC for the GDI calls below
        let bmp = CreateCompatibleBitmap(screen, w, h);
        let old = SelectObject(memdc, HGDIOBJ(bmp.0));
        let blt = BitBlt(memdc, 0, 0, w, h, screen, x, y, SRCCOPY);

        let mut bi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h, // negative => top-down rows
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut buf = vec![0u8; (w as usize) * (h as usize) * 4];
        let rows = GetDIBits(
            memdc,
            bmp,
            0,
            h as u32,
            Some(buf.as_mut_ptr() as *mut core::ffi::c_void),
            &mut bi,
            DIB_RGB_COLORS,
        );

        // best-effort cleanup
        SelectObject(memdc, old);
        let _ = DeleteObject(HGDIOBJ(bmp.0));
        let _ = DeleteDC(memdc);
        ReleaseDC(None, screen);

        blt.map_err(|e| anyhow!("BitBlt failed: {e}"))?;
        if rows == 0 {
            return Err(anyhow!("GetDIBits returned no rows"));
        }

        // GDI gives BGRA with alpha 0; swap to RGBA and force opaque.
        for px in buf.chunks_exact_mut(4) {
            px.swap(0, 2);
            px[3] = 255;
        }

        let img = image::RgbaImage::from_raw(w as u32, h as u32, buf)
            .ok_or_else(|| anyhow!("capture buffer/size mismatch"))?;
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png)?;
        Ok(out.into_inner())
    }
}
