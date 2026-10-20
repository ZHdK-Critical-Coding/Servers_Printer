// Turns PNG / JPEG data into black and white dots for the printers
use image::imageops::FilterType;
use image::{GrayImage, Luma};

#[derive(Clone, Copy)]
pub enum Dither {
    // Hard black and white, good for drawings
    Threshold,
    // Error diffusion, good for photos and grey tones
    FloydSteinberg,
}

// dots[y * width + x] is true for a black dot
pub struct Dots {
    pub width: usize,
    pub height: usize,
    pub dots: Vec<bool>,
}

// Decodes the image, composites transparent pixels onto white and returns it in grey
pub fn decode_gray(data: &[u8]) -> Result<GrayImage, String> {
    let rgba = image::load_from_memory(data)
        .map_err(|e| format!("Image could not be read: {e}"))?
        .to_rgba8();
    let mut gray = GrayImage::new(rgba.width(), rgba.height());
    for (x, y, p) in rgba.enumerate_pixels() {
        let [r, g, b, a] = p.0;
        let a = a as f32 / 255.0;
        let v = (0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32) * a + 255.0 * (1.0 - a);
        gray.put_pixel(x, y, Luma([v.round().clamp(0.0, 255.0) as u8]));
    }
    Ok(gray)
}

// `size(width, height)` returns the size in printer dots for an image of the given size
pub fn to_dots(
    data: &[u8],
    size: impl Fn(u32, u32) -> (u32, u32),
    dither: Dither,
    threshold: f32,
) -> Result<Dots, String> {
    let gray = decode_gray(data)?;
    let (w, h) = size(gray.width(), gray.height());
    let resized = image::imageops::resize(&gray, w.max(1), h.max(1), FilterType::Lanczos3);
    let (width, height) = (resized.width() as usize, resized.height() as usize);

    let mut values: Vec<f32> = resized.pixels().map(|p| p.0[0] as f32).collect();
    let mut dots = vec![false; width * height];
    for y in 0..height {
        for x in 0..width {
            let i = y * width + x;
            let black = values[i] < threshold;
            dots[i] = black;

            if let Dither::FloydSteinberg = dither {
                // Spread the error to the neighbouring pixels
                let error = values[i] - if black { 0.0 } else { 255.0 };
                if x + 1 < width {
                    values[i + 1] += error * 7.0 / 16.0;
                }
                if y + 1 < height {
                    if x > 0 {
                        values[i + width - 1] += error * 3.0 / 16.0;
                    }
                    values[i + width] += error * 5.0 / 16.0;
                    if x + 1 < width {
                        values[i + width + 1] += error / 16.0;
                    }
                }
            }
        }
    }
    Ok(Dots { width, height, dots })
}

// Builds ESC * bit image bands: every band is `rows` dots high (8 or 24),
// one column is `rows / 8` bytes, most significant bit on top
pub fn to_bands(image: &Dots, rows: usize) -> Vec<Vec<u8>> {
    let bytes_per_column = rows / 8;
    let mut bands = Vec::new();
    for top in (0..image.height).step_by(rows) {
        let mut band = vec![0u8; image.width * bytes_per_column];
        for x in 0..image.width {
            for bit in 0..rows {
                let y = top + bit;
                if y < image.height && image.dots[y * image.width + x] {
                    band[x * bytes_per_column + (bit >> 3)] |= 0x80 >> (bit & 7);
                }
            }
        }
        bands.push(band);
    }
    bands
}

// PNG of the dots, stretched by `scale_y` to show the real proportions on paper
pub fn preview(image: &Dots, scale_y: f32) -> Vec<u8> {
    let mut gray = GrayImage::new(image.width as u32, image.height as u32);
    for (i, black) in image.dots.iter().enumerate() {
        let (x, y) = ((i % image.width) as u32, (i / image.width) as u32);
        gray.put_pixel(x, y, Luma([if *black { 0 } else { 255 }]));
    }
    let height = ((image.height as f32 * scale_y).round() as u32).max(1);
    let stretched = image::imageops::resize(&gray, gray.width(), height, FilterType::Nearest);
    let mut png = Vec::new();
    let _ = stretched.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png);
    png
}
