use crate::gbmode::GbMode;
use std::cmp::Ordering;

const VRAM_SIZE: usize = 0x4000;
const VOAM_SIZE: usize = 0xA0;
pub const SCREEN_W: usize = 160;
pub const SCREEN_H: usize = 144;

/// Dot position of line 0 right after the LCD is switched on.
const LCD_ON_START_DOT: u32 = 4;

/// Per-line RGB lookup tables (see `GPU::line_luts`).
struct LineLuts {
    bg: [[u8; 3]; 32],
    obj: [[u8; 3]; 32],
}

fn put(line: &mut [u8], x: usize, rgb: [u8; 3]) {
    line[x * 3..x * 3 + 3].copy_from_slice(&rgb);
}

/// CGB 15-bit color (5 bits per channel) to display RGB: either plain
/// 5-to-8-bit expansion, or with the LCD color correction taken from the
/// Gambatte emulator.
fn cgb_rgb(r: u8, g: u8, b: u8, raw: bool) -> [u8; 3] {
    if raw {
        return [(r << 3) | (r >> 2), (g << 3) | (g >> 2), (b << 3) | (b >> 2)];
    }
    let (r, g, b) = (r as u32, g as u32, b as u32);
    [
        ((r * 13 + g * 2 + b) >> 1) as u8,
        ((g * 3 + b) << 1) as u8,
        ((r * 3 + g * 2 + b * 11) >> 1) as u8,
    ]
}

/// One 8-pixel row of a background or window tile.
struct TileRow {
    b1: u8,
    b2: u8,
    palnr: usize,
    xflip: bool,
    prio: bool,
}

#[derive(PartialEq, Copy, Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
enum PrioType {
    Color0,
    PrioFlag,
    Normal,
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct GPU {
    mode: u8,
    modeclock: u32,
    line: u8,
    lyc: u8,
    lcd_on: bool,
    win_tilemap: u16,
    win_on: bool,
    tilebase: u16,
    bg_tilemap: u16,
    sprite_size: u32,
    sprite_on: bool,
    lcdc0: bool,
    lyc_inte: bool,
    m0_inte: bool,
    m1_inte: bool,
    m2_inte: bool,
    scy: u8,
    scx: u8,
    winy: u8,
    winx: u8,
    wy_trigger: bool,
    wy_pos: i32,
    palbr: u8,
    pal0r: u8,
    pal1r: u8,
    palb: [u8; 4],
    pal0: [u8; 4],
    pal1: [u8; 4],
    vram: [u8; VRAM_SIZE],
    voam: [u8; VOAM_SIZE],
    cbgpal_inc: bool,
    cbgpal_ind: u8,
    cbgpal: [[[u8; 3]; 4]; 8],
    csprit_inc: bool,
    csprit_ind: u8,
    csprit: [[[u8; 3]; 4]; 8],
    vrambank: usize,
    // Two reusable frame buffers (RGB bytes). We render into the back buffer and
    // swap at start of VBlank, flagging 'updated'. This allows the front buffer
    // to be read concurrently (e.g., by UI) without copying mid-frame.
    // Not saved: they're output, and a decoded GPU takes the live one's via
    // `adopt_frame_buffers` (the next frame is fully redrawn anyway).
    #[rkyv(with = rkyv::with::Skip)]
    frame_buffers: [Vec<u8>; 2],
    #[rkyv(with = rkyv::with::Skip)]
    front: usize, // index of front buffer (0 or 1)
    /// Host display preference, not state: CGB colors get the LCD color
    /// correction (default) or are shown raw.
    #[rkyv(with = rkyv::with::Skip)]
    raw_cgb_colors: bool,
    bgprio: [PrioType; SCREEN_W],
    pub updated: bool,
    pub interrupt: u8,
    pub gbmode: GbMode,
    hblanking: bool,
    first_frame: bool,
    // Length of this line's mode 3 in dots (172 + SCX fine scroll, window
    // and sprite fetch penalties), fixed when mode 3 starts.
    mode3_len: u32,
    /// Line 0 right after the LCD is switched on has no OAM scan: it stays
    /// in mode 0 until mode 3.
    first_line_after_on: bool,
    prev_stat_signal: bool,
}

impl GPU {
    pub fn new() -> GPU {
        GPU {
            mode: 0,
            modeclock: 0,
            line: 0,
            lyc: 0,
            lcd_on: false,
            win_tilemap: 0x9C00,
            win_on: false,
            tilebase: 0x8000,
            bg_tilemap: 0x9C00,
            sprite_size: 8,
            sprite_on: false,
            lcdc0: false,
            lyc_inte: false,
            m2_inte: false,
            m1_inte: false,
            m0_inte: false,
            scy: 0,
            scx: 0,
            winy: 0,
            winx: 0,
            wy_trigger: false,
            wy_pos: -1,
            palbr: 0,
            pal0r: 0,
            pal1r: 1,
            palb: [0; 4],
            pal0: [0; 4],
            pal1: [0; 4],
            vram: [0; VRAM_SIZE],
            voam: [0; VOAM_SIZE],
            frame_buffers: [vec![0; SCREEN_W * SCREEN_H * 3], vec![0; SCREEN_W * SCREEN_H * 3]],
            front: 0,
            bgprio: [PrioType::Normal; SCREEN_W],
            updated: false,
            interrupt: 0,
            gbmode: GbMode::Classic,
            cbgpal_inc: false,
            cbgpal_ind: 0,
            cbgpal: [[[0u8; 3]; 4]; 8],
            csprit_inc: false,
            csprit_ind: 0,
            csprit: [[[0u8; 3]; 4]; 8],
            vrambank: 0,
            hblanking: false,
            first_frame: false,
            mode3_len: 172,
            first_line_after_on: false,
            raw_cgb_colors: false,
            prev_stat_signal: false,
        }
    }

    pub fn do_cycle(&mut self, ticks: u32) {
        if !self.lcd_on {
            return;
        }
        self.hblanking = false;

        let mut ticksleft = ticks;

        while ticksleft > 0 {
            let curticks = if ticksleft >= 80 { 80 } else { ticksleft };
            self.modeclock += curticks;
            ticksleft -= curticks;

            // Full line takes 114 ticks
            if self.modeclock >= 456 {
                self.modeclock -= 456;
                self.line = (self.line + 1) % 154;
                self.first_line_after_on = false;
                self.update_stat_signal();

                // This is a VBlank line
                if self.line >= 144 && self.mode != 1 {
                    self.change_mode(1);
                }
            }

            // This is a normal line
            if self.line < 144 {
                if self.modeclock <= 80 {
                    if self.mode != 2 && !self.first_line_after_on {
                        self.change_mode(2);
                    }
                } else if self.modeclock <= 80 + self.mode3_len {
                    // 252 cycles
                    if self.mode != 3 {
                        self.change_mode(3);
                    }
                } else {
                    // the remaining 204
                    if self.mode != 0 {
                        self.change_mode(0);
                    }
                }
            }
        }
    }

    /// Mode 3 length for the current line (Pan Docs "Mode 3 length"):
    /// 172 dots, plus SCX % 8 discarded pixels, plus 6 if the window is
    /// drawn, plus a per-sprite fetch penalty (calibrated against Mooneye
    /// intr_2_mode0_timing_sprites).
    fn compute_mode3_len(&self) -> u32 {
        let mut len = 172 + (self.scx & 7) as u32;
        let window = self.win_on && self.wy_trigger && self.winx <= 166;
        if window {
            len += 6;
        }
        if !self.sprite_on {
            return len;
        }
        let line = self.line as i32;
        let height = self.sprite_size as i32;
        // Background tiles whose fetch a sprite already stalled.
        let mut stalled_tiles = [false; 32];
        let mut count = 0;
        for i in 0..40 {
            let y = self.voam[i * 4] as i32 - 16;
            if line < y || line >= y + height {
                continue;
            }
            count += 1;
            if count > 10 {
                break;
            }
            let oam_x = self.voam[i * 4 + 1] as u32;
            if oam_x >= 168 {
                continue; // fully off the right edge: not fetched
            }
            // Position in fetcher coordinates (the first, partly hidden BG
            // tile starts at 0). The first sprite in a BG tile stalls that
            // tile's fetch for up to 5 dots; every sprite costs 6 more.
            let pos = oam_x + (self.scx & 7) as u32;
            let tile = (pos / 8) as usize;
            if !stalled_tiles[tile] {
                stalled_tiles[tile] = true;
                len += 5 - (pos % 8).min(5);
            }
            len += 6;
        }
        len
    }

    /// The PPU owns VRAM while drawing (mode 3): CPU access is cut off.
    pub fn vram_blocked(&self) -> bool {
        self.lcd_on && self.mode == 3
    }

    /// The PPU owns OAM while scanning and drawing (modes 2 and 3).
    pub fn oam_blocked(&self) -> bool {
        self.lcd_on && (self.mode == 2 || self.mode == 3)
    }

    /// Move the PPU to `dots` into line 0 (its position when the boot ROM
    /// hands over), with the mode that position implies.
    pub fn set_post_boot_clock(&mut self, dots: u32) {
        self.line = 0;
        self.modeclock = dots;
        self.mode = if dots <= 80 { 2 } else if dots <= 80 + 172 { 3 } else { 0 };
    }

    /// Dots until the next mode or line change, i.e. the next point where an
    /// interrupt could be raised. `u32::MAX` while the LCD is off; 0 if the
    /// mode is out of step with the clock (it will be fixed on the next step,
    /// which must not be skipped).
    pub fn ticks_until_event(&self) -> u32 {
        if !self.lcd_on {
            return u32::MAX;
        }
        let (expected_mode, next) = if self.line >= 144 {
            (1, 456)
        } else if self.modeclock <= 80 {
            (if self.first_line_after_on { 0 } else { 2 }, 81)
        } else if self.modeclock <= 80 + self.mode3_len {
            (3, 81 + self.mode3_len)
        } else {
            (0, 456)
        };
        if self.mode != expected_mode {
            return 0;
        }
        next - self.modeclock
    }

    fn update_stat_signal(&mut self) {
        // Combined STAT line: OR of all enabled sources (Pan Docs §STAT register).
        // The signal is forced low while the LCD is off, so no STAT IRQ can fire
        // from register writes (FF41/FF45) during LCD-off, and a clean baseline
        // is kept for the rising-edge check after re-enable.
        let signal = self.lcd_on
            && ((self.m0_inte && self.mode == 0)
                || (self.m1_inte && self.mode == 1)
                || (self.m2_inte && self.mode == 2)
                || (self.lyc_inte && self.line == self.lyc));
        if signal && !self.prev_stat_signal {
            self.interrupt |= 0x02;
        }
        self.prev_stat_signal = signal;
    }

    fn change_mode(&mut self, mode: u8) {
        self.mode = mode;

        match self.mode {
            0 => {
                self.renderscan();
                self.hblanking = true;
            }
            1 => {
                // Vertical blank
                self.wy_trigger = false;
                self.interrupt |= 0x01;
                // Frame complete: swap front/back so consumers see a stable frame.
                self.front ^= 1;
                self.updated = true;
                self.first_frame = false;
            }
            2 => {}
            3 => {
                if self.win_on && !self.wy_trigger && self.line == self.winy {
                    self.wy_trigger = true;
                    self.wy_pos = -1;
                }
                self.mode3_len = self.compute_mode3_len();
            }
            _ => {}
        }
        self.update_stat_signal();
    }

    pub fn rb(&self, a: u16) -> u8 {
        match a {
            0x8000..=0x9FFF => self.vram[(self.vrambank * 0x2000) | (a as usize & 0x1FFF)],
            0xFE00..=0xFE9F => self.voam[a as usize - 0xFE00],
            0xFF40 => {
                (if self.lcd_on { 0x80 } else { 0 })
                    | (if self.win_tilemap == 0x9C00 { 0x40 } else { 0 })
                    | (if self.win_on { 0x20 } else { 0 })
                    | (if self.tilebase == 0x8000 { 0x10 } else { 0 })
                    | (if self.bg_tilemap == 0x9C00 { 0x08 } else { 0 })
                    | (if self.sprite_size == 16 { 0x04 } else { 0 })
                    | (if self.sprite_on { 0x02 } else { 0 })
                    | (if self.lcdc0 { 0x01 } else { 0 })
            }
            0xFF41 => {
                0x80 | (if self.lyc_inte { 0x40 } else { 0 })
                    | (if self.m2_inte { 0x20 } else { 0 })
                    | (if self.m1_inte { 0x10 } else { 0 })
                    | (if self.m0_inte { 0x08 } else { 0 })
                    | (if self.line == self.lyc { 0x04 } else { 0 })
                    | self.mode
            }
            0xFF42 => self.scy,
            0xFF43 => self.scx,
            0xFF44 => self.line,
            0xFF45 => self.lyc,
            0xFF46 => 0, // Write only
            0xFF47 => self.palbr,
            0xFF48 => self.pal0r,
            0xFF49 => self.pal1r,
            0xFF4A => self.winy,
            0xFF4B => self.winx,
            0xFF4C => 0xFF,
            0xFF4E => 0xFF,
            // CGB hardware running a DMG cart: VBK reads as bank 0, fixed, and
            // the palette index registers keep where the boot ROM's
            // colorization writes left them (they're locked afterwards).
            0xFF4F if self.gbmode == GbMode::ColorAsClassic => 0xFE,
            0xFF68 if self.gbmode == GbMode::ColorAsClassic => 0xC8,
            0xFF6A if self.gbmode == GbMode::ColorAsClassic => 0xD0,
            0xFF4F..=0xFF6B if self.gbmode != GbMode::Color => 0xFF,
            0xFF4F => self.vrambank as u8 | 0xFE,
            0xFF68 => 0x40 | self.cbgpal_ind | (if self.cbgpal_inc { 0x80 } else { 0 }),
            0xFF69 => {
                let palnum = (self.cbgpal_ind >> 3) as usize;
                let colnum = ((self.cbgpal_ind >> 1) & 0x3) as usize;
                if self.cbgpal_ind & 0x01 == 0x00 {
                    self.cbgpal[palnum][colnum][0] | ((self.cbgpal[palnum][colnum][1] & 0x07) << 5)
                } else {
                    ((self.cbgpal[palnum][colnum][1] & 0x18) >> 3)
                        | (self.cbgpal[palnum][colnum][2] << 2)
                }
            }
            0xFF6A => 0x40 | self.csprit_ind | (if self.csprit_inc { 0x80 } else { 0 }),
            0xFF6B => {
                let palnum = (self.csprit_ind >> 3) as usize;
                let colnum = ((self.csprit_ind >> 1) & 0x3) as usize;
                if self.csprit_ind & 0x01 == 0x00 {
                    self.csprit[palnum][colnum][0] | ((self.csprit[palnum][colnum][1] & 0x07) << 5)
                } else {
                    ((self.csprit[palnum][colnum][1] & 0x18) >> 3)
                        | (self.csprit[palnum][colnum][2] << 2)
                }
            }
            _ => 0xFF,
        }
    }

    fn rbvram0(&self, a: u16) -> u8 {
        if a < 0x8000 || a >= 0xA000 {
            panic!("Shouldn't have used rbvram0");
        }
        self.vram[a as usize & 0x1FFF]
    }
    fn rbvram1(&self, a: u16) -> u8 {
        if a < 0x8000 || a >= 0xA000 {
            panic!("Shouldn't have used rbvram1");
        }
        self.vram[0x2000 + (a as usize & 0x1FFF)]
    }

    pub fn wb(&mut self, a: u16, v: u8) {
        match a {
            0x8000..=0x9FFF => self.vram[(self.vrambank * 0x2000) | (a as usize & 0x1FFF)] = v,
            0xFE00..=0xFE9F => self.voam[a as usize - 0xFE00] = v,
            0xFF40 => {
                let orig_lcd_on = self.lcd_on;
                self.lcd_on = v & 0x80 == 0x80;
                self.win_tilemap = if v & 0x40 == 0x40 { 0x9C00 } else { 0x9800 };
                self.win_on = v & 0x20 == 0x20;
                self.tilebase = if v & 0x10 == 0x10 { 0x8000 } else { 0x8800 };
                self.bg_tilemap = if v & 0x08 == 0x08 { 0x9C00 } else { 0x9800 };
                self.sprite_size = if v & 0x04 == 0x04 { 16 } else { 8 };
                self.sprite_on = v & 0x02 == 0x02;
                self.lcdc0 = v & 0x01 == 0x01;
                if orig_lcd_on && !self.lcd_on {
                    self.modeclock = 0;
                    self.line = 0;
                    self.mode = 0;
                    self.wy_trigger = false;
                    self.first_frame = true;
                    self.clear_screen();
                    // lcd_on is now false, so this forces prev_stat_signal low,
                    // giving a clean rising-edge baseline for the next re-enable.
                    self.update_stat_signal();
                }
                if !orig_lcd_on && self.lcd_on {
                    // Enter mode 0 silently — no STAT IRQ on LCD-enable (hardware behaviour).
                    // Real hardware starts in mode 0 on enable before the first M2 scan;
                    // firing a STAT IRQ here breaks Pokemon Crystal, DKC, and others.
                    self.mode = 0;
                    self.modeclock = LCD_ON_START_DOT;
                    self.line = 0;
                    self.first_frame = true;
                    self.first_line_after_on = true;
                    // Set prev_stat_signal = true so that update_stat_signal() below
                    // sees no rising edge (0→1) regardless of the current signal value,
                    // suppressing any spurious IRQ on the enable transition.
                    self.prev_stat_signal = true;
                    self.update_stat_signal();
                }
            }
            0xFF41 => {
                self.lyc_inte = v & 0x40 == 0x40;
                self.m2_inte = v & 0x20 == 0x20;
                self.m1_inte = v & 0x10 == 0x10;
                self.m0_inte = v & 0x08 == 0x08;
                self.update_stat_signal();
            }
            0xFF42 => self.scy = v,
            0xFF43 => self.scx = v,
            0xFF44 => {} // Read-only
            0xFF45 => {
                self.lyc = v;
                self.update_stat_signal();
            }
            0xFF46 => debug_assert!(false, "0xFF46 should be handled by MMU, not GPU"),
            0xFF47 => {
                self.palbr = v;
                self.update_pal();
            }
            0xFF48 => {
                self.pal0r = v;
                self.update_pal();
            }
            0xFF49 => {
                self.pal1r = v;
                self.update_pal();
            }
            0xFF4A => self.winy = v,
            0xFF4B => self.winx = v,
            0xFF4C => {}
            0xFF4E => {}
            0xFF4F..=0xFF6B if self.gbmode != GbMode::Color => {}
            0xFF4F => self.vrambank = (v & 0x01) as usize,
            0xFF68 => {
                self.cbgpal_ind = v & 0x3F;
                self.cbgpal_inc = v & 0x80 == 0x80;
            }
            0xFF69 => {
                let palnum = (self.cbgpal_ind >> 3) as usize;
                let colnum = ((self.cbgpal_ind >> 1) & 0x03) as usize;
                if self.cbgpal_ind & 0x01 == 0x00 {
                    self.cbgpal[palnum][colnum][0] = v & 0x1F;
                    self.cbgpal[palnum][colnum][1] =
                        (self.cbgpal[palnum][colnum][1] & 0x18) | (v >> 5);
                } else {
                    self.cbgpal[palnum][colnum][1] =
                        (self.cbgpal[palnum][colnum][1] & 0x07) | ((v & 0x3) << 3);
                    self.cbgpal[palnum][colnum][2] = (v >> 2) & 0x1F;
                }
                if self.cbgpal_inc {
                    self.cbgpal_ind = (self.cbgpal_ind + 1) & 0x3F;
                };
            }
            0xFF6A => {
                self.csprit_ind = v & 0x3F;
                self.csprit_inc = v & 0x80 == 0x80;
            }
            0xFF6B => {
                let palnum = (self.csprit_ind >> 3) as usize;
                let colnum = ((self.csprit_ind >> 1) & 0x03) as usize;
                if self.csprit_ind & 0x01 == 0x00 {
                    self.csprit[palnum][colnum][0] = v & 0x1F;
                    self.csprit[palnum][colnum][1] =
                        (self.csprit[palnum][colnum][1] & 0x18) | (v >> 5);
                } else {
                    self.csprit[palnum][colnum][1] =
                        (self.csprit[palnum][colnum][1] & 0x07) | ((v & 0x3) << 3);
                    self.csprit[palnum][colnum][2] = (v >> 2) & 0x1F;
                }
                if self.csprit_inc {
                    self.csprit_ind = (self.csprit_ind + 1) & 0x3F;
                };
            }
            _ => debug_assert!(false, "GPU wb called with unhandled address {:04X}", a),
        }
    }

    fn clear_screen(&mut self) {
        // LCD off shows white. Fill both buffers so the next front_buffer() read
        // (which the GUI uses) sees the cleared frame regardless of swap timing.
        for buf in self.frame_buffers.iter_mut() {
            for v in buf.iter_mut() {
                *v = 255;
            }
        }
        self.updated = true;
    }

    fn update_pal(&mut self) {
        for i in 0..4 {
            self.palb[i] = GPU::get_monochrome_pal_val(self.palbr, i);
            self.pal0[i] = GPU::get_monochrome_pal_val(self.pal0r, i);
            self.pal1[i] = GPU::get_monochrome_pal_val(self.pal1r, i);
        }
    }

    fn get_monochrome_pal_val(value: u8, index: usize) -> u8 {
        match (value >> (2 * index)) & 0x03 {
            0 => 255,
            1 => 192,
            2 => 96,
            _ => 0,
        }
    }

    fn renderscan(&mut self) {
        if self.first_frame {
            // The first frame from when lcd_on is set should not be drawn.
            return;
        }

        // Render straight into this line of the back buffer. Taking the Vec
        // out (free) lets the draw functions borrow `self` alongside it.
        let back = self.front ^ 1;
        let mut fb = std::mem::take(&mut self.frame_buffers[back]);
        let start = self.line as usize * SCREEN_W * 3;
        let line = &mut fb[start..start + SCREEN_W * 3];
        line.fill(255);
        // A pixel the BG/window never draws counts as BG color 0, so
        // "behind BG" sprites still show over it.
        self.bgprio = [PrioType::Color0; SCREEN_W];

        let luts = self.line_luts();
        self.draw_bg(line, &luts);
        self.draw_sprites(line, &luts);
        self.frame_buffers[back] = fb;
    }

    /// RGB for every palette entry, built once per line instead of per pixel.
    /// Index = palette * 4 + color number. DMG: BG uses 0-3, OBP0 0-3, OBP1 4-7.
    fn line_luts(&self) -> LineLuts {
        let mut luts = LineLuts { bg: [[0; 3]; 32], obj: [[0; 3]; 32] };
        if self.gbmode == GbMode::Color {
            for p in 0..8 {
                for c in 0..4 {
                    let [r, g, b] = self.cbgpal[p][c];
                    luts.bg[p * 4 + c] = cgb_rgb(r, g, b, self.raw_cgb_colors);
                    let [r, g, b] = self.csprit[p][c];
                    luts.obj[p * 4 + c] = cgb_rgb(r, g, b, self.raw_cgb_colors);
                }
            }
        } else {
            for c in 0..4 {
                luts.bg[c] = [self.palb[c]; 3];
                luts.obj[c] = [self.pal0[c]; 3];
                luts.obj[4 + c] = [self.pal1[c]; 3];
            }
        }
        luts
    }

    fn draw_bg(&mut self, line: &mut [u8], luts: &LineLuts) {
        let drawbg = self.gbmode == GbMode::Color || self.lcdc0;

        let wx_trigger = self.winx <= 166;
        let winy = if self.win_on && self.wy_trigger && wx_trigger {
            self.wy_pos += 1;
            self.wy_pos
        } else {
            -1
        };

        // DMG: LCDC.0 = 0 blanks both background and window.
        if (winy < 0 || self.gbmode != GbMode::Color) && !drawbg {
            return;
        }

        // The window covers the screen from x = WX - 7 to the right edge.
        let win_start = if winy >= 0 {
            (self.winx as i32 - 7).max(0) as usize
        } else {
            SCREEN_W
        };

        // Background, left of the window. One tile fetch per 8 pixels.
        if drawbg {
            let bgy = self.scy.wrapping_add(self.line);
            let tiley = (bgy as u16 >> 3) & 31;
            let mut x = 0;
            while x < win_start {
                let bgx = self.scx as usize + x;
                let pixelx = bgx & 7;
                let tilex = (bgx as u16 >> 3) & 31;
                let row = self.fetch_tile_row(self.bg_tilemap, tiley, tilex, bgy as u16 & 7);
                let n = (8 - pixelx).min(win_start - x);
                for i in 0..n {
                    self.put_bg_pixel(line, luts, x + i, &row, pixelx + i);
                }
                x += n;
            }
        }

        // Window.
        if win_start < SCREEN_W {
            let tiley = (winy as u16 >> 3) & 31;
            let pixely = winy as u16 & 0x07;
            let mut x = win_start;
            while x < SCREEN_W {
                let wx = (x as i32 - (self.winx as i32 - 7)) as usize;
                let pixelx = wx & 7;
                let row = self.fetch_tile_row(self.win_tilemap, tiley, (wx >> 3) as u16, pixely);
                let n = (8 - pixelx).min(SCREEN_W - x);
                for i in 0..n {
                    self.put_bg_pixel(line, luts, x + i, &row, pixelx + i);
                }
                x += n;
            }
        }
    }

    /// Fetch one 8-pixel row of a BG/window tile: map entry, CGB attributes
    /// and the two bitplanes.
    fn fetch_tile_row(&self, tilemapbase: u16, tiley: u16, tilex: u16, pixely: u16) -> TileRow {
        let mapaddr = tilemapbase + tiley * 32 + tilex;
        let tilenr: u8 = self.rbvram0(mapaddr);

        let (palnr, vram1, xflip, yflip, prio) = if self.gbmode == GbMode::Color {
            let flags = self.rbvram1(mapaddr) as usize;
            (
                flags & 0x07,
                flags & (1 << 3) != 0,
                flags & (1 << 5) != 0,
                flags & (1 << 6) != 0,
                flags & (1 << 7) != 0,
            )
        } else {
            (0, false, false, false, false)
        };

        let tileaddress = self.tilebase
            + (if self.tilebase == 0x8000 {
                tilenr as u16
            } else {
                (tilenr as i8 as i16 + 128) as u16
            }) * 16;

        let a0 = match yflip {
            false => tileaddress + (pixely * 2),
            true => tileaddress + (14 - (pixely * 2)),
        };

        let (b1, b2) = match vram1 {
            false => (self.rbvram0(a0), self.rbvram0(a0 + 1)),
            true => (self.rbvram1(a0), self.rbvram1(a0 + 1)),
        };

        TileRow { b1, b2, palnr, xflip, prio }
    }

    /// Draw pixel `pixelx` (0-7, before flipping) of `row` at screen column `x`.
    fn put_bg_pixel(
        &mut self,
        line: &mut [u8],
        luts: &LineLuts,
        x: usize,
        row: &TileRow,
        pixelx: usize,
    ) {
        let xbit = if row.xflip { pixelx } else { 7 - pixelx };
        let colnr = ((row.b1 >> xbit) & 1) as usize | ((((row.b2 >> xbit) & 1) as usize) << 1);

        self.bgprio[x] = if colnr == 0 {
            PrioType::Color0
        } else if row.prio {
            PrioType::PrioFlag
        } else {
            PrioType::Normal
        };
        put(line, x, luts.bg[row.palnr * 4 + colnr]);
    }

    fn draw_sprites(&mut self, out: &mut [u8], luts: &LineLuts) {
        if !self.sprite_on {
            return;
        }

        let line = self.line as i32;
        let sprite_size = self.sprite_size as i32;

        let mut sprites_to_draw = [(0, 0, 0); 10];
        let mut sidx = 0;
        for index in 0..40 {
            let base = (index as usize) * 4;
            let spritey = self.voam[base] as i32 - 16;
            if line < spritey || line >= spritey + sprite_size {
                continue;
            }
            let spritex = self.voam[base + 1] as i32 - 8;
            sprites_to_draw[sidx] = (spritex, spritey, index);
            sidx += 1;
            if sidx >= 10 {
                break;
            }
        }
        if self.gbmode == GbMode::Color {
            sprites_to_draw[..sidx].sort_unstable_by(cgb_sprite_order);
        } else {
            sprites_to_draw[..sidx].sort_unstable_by(dmg_sprite_order);
        }

        // Sprites are drawn highest-priority first. The first opaque sprite
        // pixel claims its column even when it ends up hidden behind the BG,
        // so a lower-priority sprite can never show through it.
        let mut claimed = [false; SCREEN_W];
        for &(spritex, spritey, i) in sprites_to_draw[..sidx].iter().rev() {
            if spritex < -7 || spritex >= (SCREEN_W as i32) {
                continue;
            }

            let base = (i as usize) * 4;
            let tilenum = (self.voam[base + 2]
                & (if self.sprite_size == 16 { 0xFE } else { 0xFF }))
                as u16;
            let flags = self.voam[base + 3] as usize;
            let usepal1: bool = flags & (1 << 4) != 0;
            let xflip: bool = flags & (1 << 5) != 0;
            let yflip: bool = flags & (1 << 6) != 0;
            let belowbg: bool = flags & (1 << 7) != 0;
            let c_palnr = flags & 0x07;
            let c_vram1: bool = flags & (1 << 3) != 0;

            let tiley: u16 = if yflip {
                (sprite_size - 1 - (line - spritey)) as u16
            } else {
                (line - spritey) as u16
            };

            let tileaddress = 0x8000u16 + tilenum * 16 + tiley * 2;
            let (b1, b2) = if c_vram1 && self.gbmode == GbMode::Color {
                (self.rbvram1(tileaddress), self.rbvram1(tileaddress + 1))
            } else {
                (self.rbvram0(tileaddress), self.rbvram0(tileaddress + 1))
            };

            'xloop: for x in 0..8 {
                if spritex + x < 0 || spritex + x >= (SCREEN_W as i32) {
                    continue;
                }

                let xbit = 1 << (if xflip { x } else { 7 - x } as u32);
                let colnr =
                    (if b1 & xbit != 0 { 1 } else { 0 }) | (if b2 & xbit != 0 { 2 } else { 0 });
                if colnr == 0 {
                    continue;
                }
                let px = (spritex + x) as usize;
                if claimed[px] {
                    continue;
                }
                claimed[px] = true;

                if self.gbmode == GbMode::Color {
                    if self.lcdc0
                        && (self.bgprio[(spritex + x) as usize] == PrioType::PrioFlag
                            || (belowbg && self.bgprio[(spritex + x) as usize] != PrioType::Color0))
                    {
                        continue 'xloop;
                    }
                    put(out, px, luts.obj[c_palnr * 4 + colnr]);
                } else {
                    if belowbg && self.bgprio[(spritex + x) as usize] != PrioType::Color0 {
                        continue 'xloop;
                    }
                    put(out, px, luts.obj[(usepal1 as usize) * 4 + colnr]);
                }
            }
        }
    }

    pub fn may_hdma(&self) -> bool {
        return self.hblanking;
    }
}

impl GPU {
    /// Take over `live`'s frame buffers after this GPU was decoded from a save
    /// state (decoded buffers are empty).
    pub fn adopt_frame_buffers(&mut self, live: &mut GPU) {
        std::mem::swap(&mut self.frame_buffers, &mut live.frame_buffers);
        self.front = live.front;
        self.raw_cgb_colors = live.raw_cgb_colors;
    }

    pub fn set_raw_cgb_colors(&mut self, raw: bool) {
        self.raw_cgb_colors = raw;
    }

    /// Returns the current front (display) frame buffer.
    pub fn front_buffer(&self) -> &[u8] { &self.frame_buffers[self.front] }
}

// Functions to determine the order of sprites. Input is a tuple x-coord, OAM position
// These function ensures that sprites with a higher priority are 'larger'
fn dmg_sprite_order(a: &(i32, i32, u8), b: &(i32, i32, u8)) -> Ordering {
    // DMG order: prioritize on x-coord, and then by OAM position.
    if a.0 != b.0 {
        return b.0.cmp(&a.0);
    }
    return b.2.cmp(&a.2);
}

fn cgb_sprite_order(a: &(i32, i32, u8), b: &(i32, i32, u8)) -> Ordering {
    // CGB order: only prioritize based on OAM position.
    return b.2.cmp(&a.2);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// While the LCD is off, writes to STAT (FF41) or LYC (FF45) must NOT
    /// generate a STAT IRQ — even when lyc_inte && line == lyc holds.
    #[test]
    fn stat_irq_does_not_fire_while_lcd_off() {
        let mut gpu = GPU::new();
        // Reaffirm LCD off (no-op since both orig and new are off).
        gpu.wb(0xFF40, 0x00);
        assert_eq!(gpu.interrupt, 0, "no IRQ from disabling LCD");
        // line and lyc both start at 0; enable LYC interrupt, then set LYC=0.
        gpu.wb(0xFF41, 0x40); // bit 6 = lyc_inte
        gpu.wb(0xFF45, 0x00); // LYC = 0
        assert_eq!(
            gpu.interrupt & 0x02,
            0,
            "STAT IRQ must not fire while LCD is off, even with lyc_inte && line==lyc"
        );
    }

    /// On LCD disable, prev_stat_signal must reset to false so a fresh signal
    /// after re-enable computes its rising edge from a clean baseline.
    #[test]
    fn lcd_disable_resets_prev_stat_signal() {
        let mut gpu = GPU::new();
        gpu.wb(0xFF40, 0x80); // LCD on
        gpu.wb(0xFF41, 0x08); // m0_inte; mode starts at 0 → signal high, prev=true
        assert!(gpu.prev_stat_signal, "signal should be latched high while on");
        gpu.wb(0xFF40, 0x00); // LCD off
        assert!(
            !gpu.prev_stat_signal,
            "LCD disable must reset prev_stat_signal"
        );
    }

    /// Two sprites overlap; the higher-priority one (lower OAM index, same X)
    /// is flagged behind-BG and the BG pixel is opaque. Hardware picks the
    /// higher-priority sprite first and then hides it, so the BG must show —
    /// the lower-priority sprite must not leak through (dmg-acid2).
    #[test]
    fn hidden_high_priority_sprite_masks_lower_sprites() {
        let mut gpu = GPU::new();
        gpu.wb(0xFF40, 0x93); // LCD, BG, sprites on; tile data at 0x8000
        gpu.wb(0xFF47, 0xE4); // BGP identity
        gpu.wb(0xFF48, 0xE4); // OBP0 identity
        gpu.first_frame = false;
        gpu.line = 0;
        for a in 0x8000..0x8010 {
            gpu.wb(a, 0xFF); // tile 0 (BG): color 3 everywhere
        }
        for a in (0x8010..0x8020).step_by(2) {
            gpu.wb(a, 0xFF); // tile 1 (sprites): color 1 everywhere
        }
        // OAM 0: behind BG. OAM 1: normal. Same position.
        for (i, flags) in [(0u16, 0x80u8), (1, 0x00)] {
            let base = 0xFE00 + i * 4;
            gpu.wb(base, 16);
            gpu.wb(base + 1, 8);
            gpu.wb(base + 2, 1);
            gpu.wb(base + 3, flags);
        }
        gpu.renderscan();
        let back = &gpu.frame_buffers[gpu.front ^ 1];
        assert_eq!(back[0], 0, "BG color 3 should win over both sprites");
    }

    /// DMG with LCDC.0 = 0: neither BG nor window is drawn (white).
    #[test]
    fn dmg_lcdc0_off_hides_window() {
        let mut gpu = GPU::new();
        gpu.wb(0xFF40, 0xB0); // LCD on, window on, tile data 0x8000, BG off
        gpu.wb(0xFF47, 0xE4);
        gpu.wb(0xFF4A, 0); // WY
        gpu.wb(0xFF4B, 7); // WX
        gpu.first_frame = false;
        gpu.line = 0;
        gpu.wy_trigger = true;
        for a in 0x8000..0x8010 {
            gpu.wb(a, 0xFF);
        }
        gpu.renderscan();
        let back = &gpu.frame_buffers[gpu.front ^ 1];
        assert_eq!(back[0], 255, "window must be blanked when LCDC.0 = 0 on DMG");
    }

    #[test]
    fn raw_cgb_colors_expand_5_to_8_bits() {
        assert_eq!(cgb_rgb(31, 31, 31, true), [255, 255, 255]);
        assert_eq!(cgb_rgb(0, 16, 1, true), [0, 132, 8]);
        assert_ne!(cgb_rgb(31, 0, 0, false), cgb_rgb(31, 0, 0, true), "correction mixes channels");
    }
}
