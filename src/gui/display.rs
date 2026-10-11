//! Frame upload, screenshots, FPS meter, and window sizing.

use super::*;

#[derive(Default)]
pub struct RenderOptions {
    pub linear_interpolation: bool,
}

pub(super) struct FpsMeter {
    pub(super) times: VecDeque<Instant>,
    pub(super) cap: usize,
}

impl FpsMeter {
    pub(super) fn new() -> Self {
        Self { times: VecDeque::with_capacity(120), cap: 120 }
    }
    pub(super) fn record(&mut self) {
        if self.times.len() == self.cap {
            self.times.pop_front();
        }
        self.times.push_back(Instant::now());
    }
    pub(super) fn fps(&self) -> f32 {
        if self.times.len() < 2 {
            return 0.0;
        }
        let span = self
            .times
            .back()
            .unwrap()
            .duration_since(*self.times.front().unwrap())
            .as_secs_f32();
        if span <= 0.0 {
            0.0
        } else {
            (self.times.len() - 1) as f32 / span
        }
    }
}

pub(super) fn apply_window_mode(window: &winit::window::Window, scale: u32, fullscreen: bool) {
    if fullscreen {
        window.set_fullscreen(Some(Fullscreen::Borderless(None)));
    } else {
        window.set_fullscreen(None);
        set_window_size(window, scale);
    }
}

pub(super) fn upload_screen(texture: &mut glium::texture::texture2d::Texture2d, datavec: &[u8]) {
    let rawimage2d = glium::texture::RawImage2d {
        data: std::borrow::Cow::Borrowed(datavec),
        width: rust_gbe::SCREEN_W as u32,
        height: rust_gbe::SCREEN_H as u32,
        format: glium::texture::ClientFormat::U8U8U8,
    };
    texture.write(
        glium::Rect {
            left: 0,
            bottom: 0,
            width: rust_gbe::SCREEN_W as u32,
            height: rust_gbe::SCREEN_H as u32,
        },
        rawimage2d,
    );
}

/// If `apply` is true, remap the four DMG grayscale shades in `datavec` via `pal` using `scratch`
/// as a reusable buffer; otherwise upload the bytes directly.
pub(super) fn upload_frame_with_palette(
    texture: &mut glium::texture::texture2d::Texture2d,
    datavec: &[u8],
    apply: bool,
    pal: &DmgPalette,
    scratch: &mut Vec<u8>,
) {
    if apply {
        scratch.clear();
        scratch.extend_from_slice(datavec);
        apply_dmg_palette(scratch, pal);
        upload_screen(texture, scratch);
    } else {
        upload_screen(texture, datavec);
    }
}

/// Write the current frame as `<rom>-<date>-<time>.png` next to the ROM.
/// `palette` maps DMG shades the same way the display does.
pub(super) fn save_screenshot(
    rom_path: &std::path::Path,
    frame: &[u8],
    palette: Option<&DmgPalette>,
) -> Result<PathBuf, String> {
    let mut rgb = frame.to_vec();
    if let Some(p) = palette {
        apply_dmg_palette(&mut rgb, p);
    }
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let t = local_datetime(secs);
    let stem = rom_path.file_stem().unwrap_or_default().to_string_lossy();
    let path = rom_path.with_file_name(format!(
        "{stem}-{:04}{:02}{:02}-{:02}{:02}{:02}.png",
        t.year(),
        t.month() as u8,
        t.day(),
        t.hour(),
        t.minute(),
        t.second()
    ));
    let file = std::fs::File::create(&path).map_err(|e| e.to_string())?;
    let mut encoder = png::Encoder::new(
        std::io::BufWriter::new(file),
        rust_gbe::SCREEN_W as u32,
        rust_gbe::SCREEN_H as u32,
    );
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .and_then(|mut w| w.write_image_data(&rgb))
        .map_err(|e| e.to_string())?;
    Ok(path)
}

pub(super) fn position_on_a_monitor(window: &winit::window::Window, x: i32, y: i32) -> bool {
    window.available_monitors().any(|m| {
        let (pos, size) = (m.position(), m.size());
        x >= pos.x
            && y >= pos.y
            && x < pos.x + size.width as i32
            && y < pos.y + size.height as i32
    })
}

pub(super) fn set_window_size(window: &winit::window::Window, scale: u32) {
    // Add extra height for the menu bar (approximately 30 pixels at 1x scale)
    let menu_bar_height = 30;
    let _ = window.request_inner_size(winit::dpi::LogicalSize::<u32>::from((
        rust_gbe::SCREEN_W as u32 * scale,
        rust_gbe::SCREEN_H as u32 * scale + menu_bar_height,
    )));
}

/// Fit the GB frame into `avail_w` x `avail_h` preserving the 160:144 aspect.
/// Returns (left, bottom, width, height) of the centered destination rect.
/// With `integer_scaling`, the scale is floored to a whole multiple; below
/// native size it falls back to a fractional aspect-preserving shrink.
pub(super) fn fit_rect(avail_w: u32, avail_h: u32, integer_scaling: bool) -> (u32, u32, u32, u32) {
    let (nw, nh) = (rust_gbe::SCREEN_W as f64, rust_gbe::SCREEN_H as f64);
    let mut scale = (avail_w as f64 / nw).min(avail_h as f64 / nh);
    if integer_scaling && scale >= 1.0 {
        scale = scale.floor();
    }
    let w = (nw * scale) as u32;
    let h = (nh * scale) as u32;
    ((avail_w.saturating_sub(w)) / 2, (avail_h.saturating_sub(h)) / 2, w, h)
}
