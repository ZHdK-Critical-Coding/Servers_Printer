// Places an image on a one-page PDF for CUPS printers
use flate2::Compression;
use flate2::write::ZlibEncoder;
use serde::{Deserialize, Serialize};
use std::io::Write;

const PT_PER_MM: f32 = 72.0 / 25.4;

// Job options of the CUPS protocol, sent as JSON by the sketch. Missing fields keep their value.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct PageOptions {
    // "A3", "A4", "A5", "A6", "Letter", "Legal"
    pub media: String,
    // "auto" (follows the image), "portrait" or "landscape"
    pub orientation: String,
    // true: scale the image to fill the page inside the margin. false: print it at `dpi`.
    pub fit: bool,
    // Pixels per inch when `fit` is false. 96 is the size of the canvas on screen.
    pub dpi: f32,
    // White border in millimeters
    pub margin: f32,
    pub copies: u32,
    // true: black and white dots (Floyd-Steinberg), like on the line printer
    pub dither: bool,
}

impl Default for PageOptions {
    fn default() -> Self {
        PageOptions {
            media: "A4".into(),
            orientation: "auto".into(),
            fit: true,
            dpi: 96.0,
            margin: 10.0,
            copies: 1,
            dither: false,
        }
    }
}

// Size in millimeters, portrait
pub fn media_size(media: &str) -> Option<(f32, f32)> {
    Some(match media.to_ascii_lowercase().as_str() {
        "a3" => (297.0, 420.0),
        "a4" => (210.0, 297.0),
        "a5" => (148.0, 210.0),
        "a6" => (105.0, 148.0),
        "letter" => (215.9, 279.4),
        "legal" => (215.9, 355.6),
        _ => return None,
    })
}

pub fn image_pdf(data: &[u8], options: &PageOptions) -> Result<Vec<u8>, String> {
    let (mw, mh) = media_size(&options.media).ok_or_else(|| {
        format!("Unknown media '{}', use A3, A4, A5, A6, Letter or Legal", options.media)
    })?;

    // Image as RGB on white, or as dithered black and white
    let (iw, ih, rgb) = if options.dither {
        let dots = crate::bitmap::to_dots(
            data,
            |w, h| (w, h),
            crate::bitmap::Dither::FloydSteinberg,
            128.0,
        )?;
        let rgb = dots.dots.iter().flat_map(|&b| [if b { 0 } else { 255 }; 3]).collect::<Vec<u8>>();
        (dots.width as f32, dots.height as f32, rgb)
    } else {
        let rgba = image::load_from_memory(data)
            .map_err(|e| format!("Image could not be read: {e}"))?
            .to_rgba8();
        let mut rgb = Vec::with_capacity(rgba.len() / 4 * 3);
        for p in rgba.pixels() {
            let a = p.0[3] as f32 / 255.0;
            for c in &p.0[..3] {
                rgb.push((*c as f32 * a + 255.0 * (1.0 - a)).round() as u8);
            }
        }
        (rgba.width() as f32, rgba.height() as f32, rgb)
    };

    let landscape = match options.orientation.as_str() {
        "landscape" => true,
        "portrait" => false,
        _ => iw > ih,
    };
    let (pw, ph) = if landscape { (mh, mw) } else { (mw, mh) };
    let (pw, ph) = (pw * PT_PER_MM, ph * PT_PER_MM);
    let margin = options.margin.max(0.0) * PT_PER_MM;
    let (bw, bh) = ((pw - 2.0 * margin).max(1.0), (ph - 2.0 * margin).max(1.0));

    // Points per image pixel; images too big for the page are scaled down
    let fit = (bw / iw).min(bh / ih);
    let scale = if options.fit { fit } else { (72.0 / options.dpi.max(1.0)).min(fit) };
    let (w, h) = (iw * scale, ih * scale);
    let (x, y) = ((pw - w) / 2.0, (ph - h) / 2.0);

    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&rgb).map_err(|e| e.to_string())?;
    let pixels = encoder.finish().map_err(|e| e.to_string())?;
    let content = format!("q {w:.3} 0 0 {h:.3} {x:.3} {y:.3} cm /Im0 Do Q\n");

    let objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {pw:.3} {ph:.3}] \
             /Resources << /XObject << /Im0 5 0 R >> >> /Contents 4 0 R >>"
        )
        .into_bytes(),
        stream(format!("<< /Length {} >>", content.len()), content.as_bytes()),
        stream(
            format!(
                "<< /Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace /DeviceRGB \
                 /BitsPerComponent 8 /Filter /FlateDecode /Length {} >>",
                iw as u32,
                ih as u32,
                pixels.len()
            ),
            &pixels,
        ),
    ];

    let mut pdf = b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n".to_vec();
    let mut offsets = Vec::new();
    for (i, object) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend(format!("{} 0 obj\n", i + 1).as_bytes());
        pdf.extend(object);
        pdf.extend(b"\nendobj\n");
    }
    let xref = pdf.len();
    pdf.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for offset in offsets {
        pdf.extend(format!("{offset:010} 00000 n \n").as_bytes());
    }
    pdf.extend(
        format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1)
            .as_bytes(),
    );
    Ok(pdf)
}

fn stream(dict: String, data: &[u8]) -> Vec<u8> {
    let mut bytes = dict.into_bytes();
    bytes.extend(b"\nstream\n");
    bytes.extend(data);
    bytes.extend(b"\nendstream");
    bytes
}
