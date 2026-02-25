use crate::drivers::vga::Color;
use alloc::vec::Vec;

pub struct Bitmap {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<Color>, // 0x00RRGGBB, row-major, top-to-bottom
}

/// Decode a 24-bit uncompressed BMP.  Returns None on any format mismatch.
pub fn decode(data: &[u8]) -> Option<Bitmap> {
    if data.len() < 54 {
        return None;
    }
    if data[0] != b'B' || data[1] != b'M' {
        return None;
    }

    let pixel_offset = u32::from_le_bytes([data[10], data[11], data[12], data[13]]) as usize;
    let width = u32::from_le_bytes([data[18], data[19], data[20], data[21]]) as usize;
    let height_raw = i32::from_le_bytes([data[22], data[23], data[24], data[25]]);
    let bpp = u16::from_le_bytes([data[28], data[29]]) as usize;
    let compression = u32::from_le_bytes([data[30], data[31], data[32], data[33]]);

    if bpp != 24 || compression != 0 || width == 0 {
        return None;
    }

    let (height, bottom_up) = if height_raw < 0 {
        ((-height_raw) as usize, false)
    } else {
        (height_raw as usize, true)
    };

    if height == 0 {
        return None;
    }

    // Each row is padded to a 4-byte boundary
    let row_bytes = (width * 3 + 3) & !3;

    if data.len() < pixel_offset + row_bytes * height {
        return None;
    }

    let mut pixels = Vec::with_capacity(width * height);
    for y in 0..height {
        let src_y = if bottom_up { height - 1 - y } else { y };
        let row_start = pixel_offset + src_y * row_bytes;
        for x in 0..width {
            let b = data[row_start + x * 3] as u32;
            let g = data[row_start + x * 3 + 1] as u32;
            let r = data[row_start + x * 3 + 2] as u32;
            pixels.push((r << 16) | (g << 8) | b);
        }
    }

    Some(Bitmap { width, height, pixels })
}
