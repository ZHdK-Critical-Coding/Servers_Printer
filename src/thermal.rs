// Epson TM thermal receipt printer, ESC/POS commands
use crate::bitmap::{self, Dither, Dots};
use crate::cp437;

const ESC: u8 = 0x1b;
const GS: u8 = 0x1d;
const LF: u8 = 0x0a;

// Same as the Raspberry Pi server: density 's8' (ESC * mode 0) prints 256 dots wide with
// dots 1.6 times taller than wide, so the image is squeezed vertically. Hard threshold at 128.
const MODE: u8 = 0;
const WIDTH: u32 = 256;
const ROWS: usize = 8;
const STRETCH: f32 = 1.6;
const THRESHOLD: f32 = 128.0;
// ESC t 0 = PC437 on Epson TM printers
const CODE_PAGE: u8 = 0;

const CUT: [u8; 6] = [LF, LF, LF, GS, 0x56, 0x00]; // Feed 3 lines, full cut

// style: 'b' bold, 'u' underline, 'bu' both. w, h: 0 = normal, 1 = double ... 7 = eight times
pub fn text(text: &str, w: i64, h: i64, style: &str, cut: bool) -> Vec<u8> {
    let size = ((w.clamp(0, 7) as u8) << 4) | h.clamp(0, 7) as u8;
    let mut bytes = vec![
        ESC, 0x40, // Initialize
        ESC, 0x74, CODE_PAGE, // Code page
        ESC, 0x61, 0, // Align left
        ESC, 0x45, style.contains('b') as u8, // Bold
        ESC, 0x2d, style.contains('u') as u8, // Underline
        GS, 0x21, size, // Character size
    ];
    bytes.extend(cp437::encode(text));
    bytes.push(LF);
    if cut {
        bytes.push(LF);
        bytes.extend(CUT);
    }
    bytes
}

pub fn cut() -> Vec<u8> {
    CUT.to_vec()
}

// Always prints at the full width, squeezed vertically so the proportions stay right
pub fn image(png: &[u8]) -> Result<(Vec<u8>, Dots), String> {
    let dots = bitmap::to_dots(
        png,
        |w, h| (WIDTH, (WIDTH as f32 / w as f32 * h as f32 / STRETCH).round() as u32),
        Dither::Threshold,
        THRESHOLD,
    )?;

    let mut bytes = vec![
        ESC, 0x40, // Initialize
        ESC, 0x61, 1, // Center
        ESC, 0x33, 0, // No line spacing between the bands
    ];
    for band in bitmap::to_bands(&dots, ROWS) {
        bytes.extend([ESC, 0x2a, MODE, (dots.width & 0xff) as u8, (dots.width >> 8) as u8]);
        bytes.extend(band);
        bytes.push(LF);
    }
    bytes.extend([ESC, 0x32, ESC, 0x61, 0]); // Default line spacing, align left
    Ok((bytes, dots))
}

pub fn preview_scale() -> f32 {
    STRETCH
}
