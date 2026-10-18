//! Real app icons: the exe's own icon as a PNG data URL, like Auto-Me-Correct. The monitor
//! remembers each foreground exe's full path; the icon is extracted once and cached in
//! %LOCALAPPDATA%\Invigil\icons so it still shows after the app closes.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;

use parking_lot::Mutex;

const SIZE: i32 = 48;

fn paths() -> &'static Mutex<HashMap<String, String>> {
    static P: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    P.get_or_init(Default::default)
}

/// Called by the monitor every tick with the foreground process.
pub fn remember(exe: &str, path: &str) {
    if exe.is_empty() || path.is_empty() {
        return;
    }
    let exe = exe.to_lowercase();
    let mut p = paths().lock();
    if !p.contains_key(&exe) {
        p.insert(exe, path.to_string());
    }
}

fn cache(exe: &str) -> PathBuf {
    let mut p = dirs::data_local_dir().unwrap_or_else(|| PathBuf::from("."));
    p.push("Invigil");
    p.push("icons");
    p.push(format!("{exe}.png"));
    p
}

/// PNG data URL for `exe` (e.g. "chrome.exe"), or None if we never saw it in the foreground.
pub fn icon(exe: &str) -> Option<String> {
    let exe = exe.to_lowercase();
    if exe.is_empty() || exe.contains(['\\', '/']) {
        return None;
    }
    let file = cache(&exe);
    let png = match std::fs::read(&file) {
        Ok(b) => b,
        Err(_) => {
            let path = paths().lock().get(&exe).cloned()?;
            let b = extract(&path)?;
            let _ = std::fs::create_dir_all(file.parent()?);
            let _ = std::fs::write(&file, &b);
            b
        }
    };
    Some(format!("data:image/png;base64,{}", base64(&png)))
}

#[cfg(not(windows))]
fn extract(_path: &str) -> Option<Vec<u8>> {
    None
}

#[cfg(windows)]
fn extract(path: &str) -> Option<Vec<u8>> {
    use windows::core::PCWSTR;
    use windows::Win32::UI::Shell::SHDefExtractIconW;
    use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, HICON};

    let wide: Vec<u16> = path.encode_utf16().chain([0]).collect();
    let mut hicon = HICON::default();
    unsafe {
        SHDefExtractIconW(PCWSTR(wide.as_ptr()), 0, 0, Some(&mut hicon), None, SIZE as u32).ok().ok()?;
        let rgba = pixels(hicon);
        let _ = DestroyIcon(hicon);
        encode(&rgba?)
    }
}

#[cfg(windows)]
unsafe fn pixels(hicon: windows::Win32::UI::WindowsAndMessaging::HICON) -> Option<Vec<u8>> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Gdi::{
        DeleteObject, GetDC, GetDIBits, ReleaseDC, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
    };
    use windows::Win32::UI::WindowsAndMessaging::{GetIconInfo, ICONINFO};

    let mut info = ICONINFO::default();
    GetIconInfo(hicon, &mut info).ok()?;
    let mut bmi = BITMAPINFO::default();
    bmi.bmiHeader = BITMAPINFOHEADER {
        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: SIZE,
        biHeight: -SIZE,
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB.0,
        ..Default::default()
    };
    let n = (SIZE * SIZE) as usize;
    let mut px = vec![0u8; n * 4];
    let mut mask = vec![0u8; n * 4];
    let dc = GetDC(HWND::default());
    let ok = GetDIBits(dc, info.hbmColor, 0, SIZE as u32, Some(px.as_mut_ptr().cast()), &mut bmi, DIB_RGB_COLORS) != 0;
    let has_mask = GetDIBits(dc, info.hbmMask, 0, SIZE as u32, Some(mask.as_mut_ptr().cast()), &mut bmi, DIB_RGB_COLORS) != 0;
    ReleaseDC(HWND::default(), dc);
    let _ = DeleteObject(info.hbmColor);
    let _ = DeleteObject(info.hbmMask);
    if !ok {
        return None;
    }
    // Old icons have no alpha: use the AND mask (black = opaque) instead.
    let no_alpha = px.chunks(4).all(|p| p[3] == 0);
    for (i, p) in px.chunks_mut(4).enumerate() {
        p.swap(0, 2);
        if no_alpha {
            p[3] = if has_mask && mask[i * 4] != 0 { 0 } else { 255 };
        }
    }
    Some(px)
}

fn encode(rgba: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, SIZE as u32, SIZE as u32);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().ok()?.write_image_data(rgba).ok()?;
    Some(out)
}

fn base64(b: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(b.len().div_ceil(3) * 4);
    for c in b.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            s.push(if i <= c.len() { T[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' });
        }
    }
    s
}

#[cfg(test)]
mod tests {
    #[test]
    fn base64_pads() {
        assert_eq!(super::base64(b"Man"), "TWFu");
        assert_eq!(super::base64(b"M"), "TQ==");
    }

    #[cfg(windows)]
    #[test]
    fn extracts_notepad_icon() {
        let png = super::extract(r"C:\Windows\System32\notepad.exe").expect("icon");
        assert_eq!(&png[1..4], b"PNG");
    }
}
