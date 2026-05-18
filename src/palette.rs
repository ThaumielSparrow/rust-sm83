//! Host-side DMG palette presets. Maps the four grayscale shades produced by
//! the GPU in Classic / ColorAsClassic modes (0/96/192/255 in each channel) to
//! arbitrary RGB triples. Living in the binary keeps the library core agnostic
//! and avoids breaking rkyv save-state compatibility.

use crate::config::DmgPalettePreset;

#[derive(Clone, Copy)]
pub struct DmgPalette {
    /// colors[0] = lightest shade (mapped from 255), colors[3] = darkest (from 0).
    pub colors: [[u8; 3]; 4],
}

pub const GREEN: DmgPalette = DmgPalette {
    colors: [
        [155, 188, 15],
        [139, 172, 15],
        [48, 98, 48],
        [15, 56, 15],
    ],
};

pub const POCKET: DmgPalette = DmgPalette {
    colors: [
        [230, 230, 230],
        [170, 170, 170],
        [85, 85, 85],
        [15, 15, 15],
    ],
};

pub const LIGHT: DmgPalette = DmgPalette {
    colors: [
        [255, 255, 255],
        [180, 180, 180],
        [110, 110, 110],
        [60, 60, 60],
    ],
};

pub fn palette_for_preset(preset: DmgPalettePreset, custom: &[[u8; 3]; 4]) -> DmgPalette {
    match preset {
        DmgPalettePreset::Green => GREEN,
        DmgPalettePreset::Pocket => POCKET,
        DmgPalettePreset::Light => LIGHT,
        DmgPalettePreset::Custom => DmgPalette { colors: *custom },
    }
}

/// In-place remap of the 4 grayscale shades to the supplied palette.
/// Input buffer is interleaved RGB (3 bytes per pixel) as produced by
/// `Device::get_gpu_data()` in DMG mode.
pub fn apply_dmg_palette(buf: &mut [u8], pal: &DmgPalette) {
    for px in buf.chunks_exact_mut(3) {
        let dst = match px[0] {
            255 => pal.colors[0],
            192 => pal.colors[1],
            96 => pal.colors[2],
            _ => pal.colors[3], // 0 (darkest)
        };
        px.copy_from_slice(&dst);
    }
}
