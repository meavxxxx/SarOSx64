use crate::arch::x86_64::limine::FRAMEBUFFER_REQUEST;
use crate::sync::spinlock::SpinLock;
use core::sync::atomic::Ordering;

static FONT: &[u8] = include_bytes!("font8x16.bin");
const FONT_WIDTH: usize = 8;
const FONT_HEIGHT: usize = 16;

pub type Color = u32;

pub const BLACK: Color = 0x00_00_00;
pub const WHITE: Color = 0xFF_FF_FF;
pub const RED: Color = 0xFF_00_00;
pub const GREEN: Color = 0x00_FF_00;
pub const BLUE: Color = 0x00_00_FF;
pub const CYAN: Color = 0x00_FF_FF;
pub const MAGENTA: Color = 0xFF_00_FF;
pub const YELLOW: Color = 0xFF_FF_00;
pub const LIGHT_GRAY: Color = 0xAA_AA_AA;
pub const DARK_GRAY: Color = 0x55_55_55;
pub const LIGHT_GREEN: Color = 0x55_FF_55;
pub const LIGHT_BLUE: Color = 0x55_55_FF;

struct Screen {
    base: *mut u8,
    width: usize,
    height: usize,
    pitch: usize,
    bpp: usize,

    col: usize,
    row: usize,
    cols: usize,
    rows: usize,

    fg: Color,
    bg: Color,
}

unsafe impl Send for Screen {}

impl Screen {
    const fn uninit() -> Self {
        Self {
            base: core::ptr::null_mut(),
            width: 0,
            height: 0,
            pitch: 0,
            bpp: 4,
            col: 0,
            row: 0,
            cols: 0,
            rows: 0,
            fg: WHITE,
            bg: BLACK,
        }
    }

    fn put_pixel(&mut self, x: usize, y: usize, color: Color) {
        if x >= self.width || y >= self.height {
            return;
        }
        let offset = y * self.pitch + x * self.bpp;
        unsafe {
            let ptr = self.base.add(offset) as *mut u32;
            ptr.write_volatile(color);
        }
    }

    fn draw_char(&mut self, ch: u8, px: usize, py: usize, fg: Color, bg: Color) {
        let glyph_offset = (ch as usize) * FONT_HEIGHT;
        if glyph_offset + FONT_HEIGHT > FONT.len() {
            return;
        }

        for row in 0..FONT_HEIGHT {
            let byte = FONT[glyph_offset + row];
            for col in 0..FONT_WIDTH {
                let set = byte & (0x80 >> col) != 0;
                self.put_pixel(px + col, py + row, if set { fg } else { bg });
            }
        }
    }

    fn scroll_up(&mut self) {
        let line_bytes = FONT_HEIGHT * self.pitch;
        let total = self.height * self.pitch;

        unsafe {
            core::ptr::copy(self.base.add(line_bytes), self.base, total - line_bytes);
            core::ptr::write_bytes(self.base.add(total - line_bytes), 0, line_bytes);
        }

        if self.row > 0 {
            self.row -= 1;
        }
    }

    fn put_char(&mut self, ch: u8) {
        match ch {
            b'\n' => {
                self.col = 0;
                self.row += 1;
            }
            b'\r' => {
                self.col = 0;
            }
            8 => {
                if self.col > 0 {
                    self.col -= 1;
                    let px = self.col * FONT_WIDTH;
                    let py = self.row * FONT_HEIGHT;
                    self.draw_char(b' ', px, py, self.fg, self.bg);
                }
            }
            ch => {
                let px = self.col * FONT_WIDTH;
                let py = self.row * FONT_HEIGHT;
                self.draw_char(ch, px, py, self.fg, self.bg);
                self.col += 1;
                if self.col >= self.cols {
                    self.col = 0;
                    self.row += 1;
                }
            }
        }

        if self.row >= self.rows {
            self.scroll_up();
        }
    }

    fn clear(&mut self) {
        let total = self.height * self.pitch;
        unsafe {
            core::ptr::write_bytes(self.base, 0, total);
        }
        self.col = 0;
        self.row = 0;
    }

    fn write_str(&mut self, s: &str) {
        for b in s.bytes() {
            self.put_char(b);
        }
    }

    fn set_color(&mut self, fg: Color, bg: Color) {
        self.fg = fg;
        self.bg = bg;
    }
}

static SCREEN: SpinLock<Screen> = SpinLock::new(Screen::uninit());

pub fn init() {
    let resp = FRAMEBUFFER_REQUEST.response.load(Ordering::Relaxed);
    if resp.is_null() {
        log::warn!("No framebuffer available");
        return;
    }

    unsafe {
        let fbs = (*resp).framebuffers();
        if fbs.is_empty() {
            return;
        }

        let fb = &*fbs[0];
        let mut screen = SCREEN.lock();
        screen.base = fb.address;
        screen.width = fb.width as usize;
        screen.height = fb.height as usize;
        screen.pitch = fb.pitch as usize;
        screen.bpp = (fb.bpp / 8) as usize;
        screen.cols = fb.width as usize / FONT_WIDTH;
        screen.rows = fb.height as usize / FONT_HEIGHT;

        screen.clear();
    }

    let (w, h, bpp) = {
        let scr = SCREEN.lock();
        (scr.width, scr.height, scr.bpp * 8)
    };
    log::info!("Framebuffer: {}x{} {}bpp", w, h, bpp);
}

pub fn write_str(s: &str) {
    let mut scr = SCREEN.lock();
    if scr.base.is_null() {
        return;
    }
    // Strip ANSI/VT100 CSI escape sequences (ESC [ ... <final 0x40-0x7E>)
    // so they don't appear as garbage on the framebuffer.
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == 0x1b && i + 1 < b.len() && b[i + 1] == b'[' {
            i += 2;
            while i < b.len() && !(b[i] >= 0x40 && b[i] <= 0x7e) {
                i += 1;
            }
            i += 1; // skip final byte
        } else {
            scr.put_char(b[i]);
            i += 1;
        }
    }
}

pub fn set_color(fg: Color, bg: Color) {
    SCREEN.lock().set_color(fg, bg);
}

pub fn clear() {
    SCREEN.lock().clear();
}

use core::fmt;

struct VgaWriter;
impl fmt::Write for VgaWriter {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        write_str(s);
        Ok(())
    }
}

pub fn print_fmt(args: fmt::Arguments) {
    use fmt::Write;
    let mut w = VgaWriter;
    let _ = w.write_fmt(args);
}

#[macro_export]
macro_rules! print {
    ($($a:tt)*) => { $crate::drivers::vga::print_fmt(format_args!($($a)*)) };
}
#[macro_export]
macro_rules! println {
    ()          => { $crate::print!("\n") };
    ($($a:tt)*) => { $crate::print!("{}\n", format_args!($($a)*)) };
}
