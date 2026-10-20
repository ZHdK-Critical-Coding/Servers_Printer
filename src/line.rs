// Epson dot matrix printer (line printer), ESC/P commands
use crate::bitmap::{self, Dither, Dots};
use crate::cp437;

const ESC: u8 = 0x1b;
const CRLF: [u8; 2] = [0x0d, 0x0a];

// ESC * mode 0 = 60 dpi horizontally; vertically it is always 72 dpi (8 dots per 8/72 inch)
const MODE: u8 = 0;
const DPI: f32 = 60.0;
// At 60 dpi, 480 dots are 8 inches
const MAX_WIDTH: u32 = 480;
const THRESHOLD: f32 = 128.0;
// ESC t 1 = PC437 on Epson ESC/P printers
const CHARACTER_TABLE: u8 = 1;

// style: 'b' bold, 'u' underline, 'bu' both. w, h: 0 = normal, 1 = double width / height
pub fn text(text: &str, w: i64, h: i64, style: &str) -> Vec<u8> {
    let mut bytes = vec![
        ESC, 0x40, // Initialize
        ESC, 0x74, CHARACTER_TABLE, // Character table
        ESC, if style.contains('b') { 0x45 } else { 0x46 }, // Bold on / off
        ESC, 0x2d, style.contains('u') as u8, // Underline
        ESC, 0x57, (w > 0) as u8, // Double width
        ESC, 0x77, (h > 0) as u8, // Double height
    ];
    bytes.extend(cp437::encode(&text.replace("\r\n", "\n").replace('\n', "\r\n")));
    bytes.extend(CRLF);
    bytes
}

// direction: "down" feeds the paper out, "up" pulls it back in. One step is 1/216 inch.
pub fn feed(up: bool, steps: i64) -> Vec<u8> {
    let command = if up { 0x6a } else { 0x4a }; // ESC j / ESC J
    let mut bytes = vec![ESC, 0x40];
    let mut left = steps.max(1);
    while left > 0 {
        bytes.extend([ESC, command, left.min(255) as u8]);
        left -= 255;
    }
    bytes
}

// Dithered bit image, at most 8 inches wide
pub fn image(png: &[u8]) -> Result<(Vec<u8>, Dots), String> {
    let dots = bitmap::to_dots(
        png,
        |w, h| {
            let width = w.min(MAX_WIDTH);
            // Compensate the different horizontal and vertical resolution
            (width, (h as f32 * width as f32 / w as f32 * (72.0 / DPI)).round() as u32)
        },
        Dither::FloydSteinberg,
        THRESHOLD,
    )?;

    let mut bytes = vec![
        ESC, 0x40, // Initialize
        ESC, 0x33, 24, // Line spacing 24/216 inch = exactly one band
    ];
    for band in bitmap::to_bands(&dots, 8) {
        bytes.extend([ESC, 0x2a, MODE, (dots.width & 0xff) as u8, (dots.width >> 8) as u8]);
        bytes.extend(band);
        bytes.extend(CRLF);
    }
    bytes.extend([ESC, 0x32]); // Default line spacing
    bytes.extend(CRLF);
    bytes.extend(CRLF);
    Ok((bytes, dots))
}

pub fn preview_scale() -> f32 {
    DPI / 72.0
}
