// Minimal Game Boy GPU scaffold
// Tracks LCD mode/timing, LY, STAT, and produces a framebuffer buffer of 160x144 pixels

use crate::cpu::mmu::Memory;
use minifb::{Window, WindowOptions};

pub const SCREEN_WIDTH: usize = 160;
pub const SCREEN_HEIGHT: usize = 144;

pub struct GPU {
    // Pixel framebuffer: each pixel is a u8 palette index 0..3
    pub framebuffer: Vec<u8>,
    // LCD timing counters (in cycles)
    pub scanline_counter: u32,
    // LCD mode (0=HBlank,1=VBlank,2=OAM,3=VRAM)
    pub mode: u8,
    // Optional window for displaying output
    pub window: Option<Window>,
}

impl GPU {
    pub fn new() -> Self {
        GPU {
            framebuffer: vec![0; SCREEN_WIDTH * SCREEN_HEIGHT],
            scanline_counter: 0,
            mode: 2,
            window: None,
        }
    }

    pub fn open_window(&mut self, title: &str) {
        let mut window = Window::new(
            title,
            SCREEN_WIDTH,
            SCREEN_HEIGHT,
            WindowOptions::default(),
        )
        .unwrap();

        // limit to max ~60 fps
        window.limit_update_rate(Some(std::time::Duration::from_micros(16600)));
        self.window = Some(window);
    }

    // Present the framebuffer to the window. Converts 2-bit palette indexes to ARGB32
    pub fn present(&mut self) {
        if let Some(win) = &mut self.window {
            // expand to u32 ARGB buffer
            let mut buffer: Vec<u32> = Vec::with_capacity(SCREEN_WIDTH * SCREEN_HEIGHT);
            for &px in &self.framebuffer {
                let color = match px & 0x03 {
                    0 => 0xFFFFFFFF, // white
                    1 => 0xFFAAAAAA, // light gray
                    2 => 0xFF555555, // dark gray
                    3 => 0xFF000000, // black
                    _ => 0xFFFF00FF,
                };
                buffer.push(color);
            }
            let _ = win.update_with_buffer(&buffer, SCREEN_WIDTH, SCREEN_HEIGHT);
        }
    }

    // Called with executed cycles so GPU can advance state. Writes status flags to IO registers.
    pub fn step(&mut self, mem: &mut Memory, cycles: u8) {
        // Timing constants (in CPU cycles):
        // Mode 2 (OAM search): 80 cycles
        // Mode 3 (VRAM): 172 cycles
        // Mode 0 (HBlank): 204 cycles
        // Total per line: 456 cycles
        // VBlank lines: 10 lines (144..153)
        self.scanline_counter = self.scanline_counter.wrapping_add(cycles as u32);

        let current_line = mem.io_registers[0x44] as u8; // LY (FF44)

        if self.scanline_counter >= 456 {
            self.scanline_counter -= 456;
            // increment LY
            let new_line = current_line.wrapping_add(1);
            mem.io_registers[0x44] = new_line;

            if new_line == 144 {
                // Entering VBlank
                mem.io_registers[0x0F] |= 1 << 0; // Request VBlank interrupt (IF bit 0)
                mem.io_registers[0x41] = (mem.io_registers[0x41] & 0xFC) | 0; // set mode 0? ensure
                self.mode = 0;
                // present framebuffer when entering VBlank
                self.present();
            } else if new_line > 153 {
                // Wrap back to line 0
                mem.io_registers[0x44] = 0;
            }
        }

        // Update STAT (FF41) mode bits based on scanline_counter
        if mem.io_registers[0x44] >= 144 {
            self.mode = 1; // VBlank
        } else if self.scanline_counter < 80 {
            self.mode = 2; // OAM
        } else if self.scanline_counter < 80 + 172 {
            self.mode = 3; // VRAM
            // during VRAM we can render the current scanline into framebuffer
            let ly = mem.io_registers[0x44] as usize;
            if ly < SCREEN_HEIGHT {
                self.render_scanline(mem, ly);
            }
        } else {
            self.mode = 0; // HBlank
        }

        // Update STAT register mode bits
        mem.io_registers[0x41] = (mem.io_registers[0x41] & 0xFC) | (self.mode & 0x03);
    }

    fn render_scanline(&mut self, mem: &Memory, ly: usize) {
        // Basic background rendering using tile map 0x9800 or 0x9C00 and tile data at 0x8000
        // This is a simplified renderer that ignores scrolling, window, palettes, and sprites.
        // It maps each 8x8 tile to pixels left-to-right across the background (32 tiles wide)

        // BG Tile map select: LCDC bit 3 (0xFF40 bit 3)
        let lcdc = mem.io_registers[0x40];
        let bg_tile_map = if (lcdc & (1 << 3)) != 0 { 0x9C00 } else { 0x9800 };
        // Tile data select: LCDC bit 4 (0xFF40 bit 4)
        let tile_data_select = (lcdc & (1 << 4)) != 0;

        let tiles_per_row = 32;

        let tile_y = ly / 8;

        for tile_x in 0..tiles_per_row {
            let map_addr = bg_tile_map + (tile_y * tiles_per_row + tile_x) as u16;
            let tile_index = mem.read_byte(map_addr) as i16;

            // Determine tile data address
            let tile_addr = if tile_data_select {
                // unsigned index at 0x8000 + (index * 16)
                0x8000u16 + (tile_index as u16 * 16)
            } else {
                // signed index: 0x9000 + (i8(index) * 16)
                (0x9000u16 as i32 + (tile_index as i8 as i32) * 16) as u16
            };

            let line_in_tile = (ly % 8) as u16;
            let byte1 = mem.read_byte(tile_addr + (line_in_tile * 2) as u16);
            let byte2 = mem.read_byte(tile_addr + (line_in_tile * 2 + 1) as u16);

            for bit in 0..8 {
                let bit_index = 7 - bit;
                let hi = (byte2 >> bit_index) & 1;
                let lo = (byte1 >> bit_index) & 1;
                let palette_index = (hi << 1) | lo;

                let px = tile_x * 8 + bit;
                if px < SCREEN_WIDTH {
                    self.framebuffer[ly * SCREEN_WIDTH + px] = palette_index as u8;
                }
            }
        }
    }
}
