use std::io::Cursor;

use anyhow::{Context, Result};
use image::{DynamicImage, GenericImageView, ImageFormat, imageops::FilterType};

pub const CARD_WIDTH: u32 = 1_536;
pub const CARD_HEIGHT: u32 = 969;

#[derive(Clone)]
pub struct PreparedSkin {
    pub png: Vec<u8>,
    pub pdf: Vec<u8>,
    pub source_width: u32,
    pub source_height: u32,
}

impl PreparedSkin {
    pub fn from_image_with_focus(img: DynamicImage, focus_x: f32, focus_y: f32) -> Result<Self> {
        let (sw, sh) = img.dimensions();
        let cropped = crop_for_card(img, focus_x, focus_y);
        let resized = cropped.resize_exact(CARD_WIDTH, CARD_HEIGHT, FilterType::Lanczos3);
        let rgba = resized.to_rgba8();

        let mut png = Vec::new();
        DynamicImage::ImageRgba8(rgba)
            .write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
            .context("PNG encode failed")?;

        let pdf = png_to_pdf(&png)?;
        Ok(Self { png, pdf, source_width: sw, source_height: sh })
    }
}

/// Convert a PNG to a minimal single-page PDF with the image embedded.
/// Apple Wallet needs the .pdf version alongside the PNGs.
pub fn png_to_pdf(png_bytes: &[u8]) -> Result<Vec<u8>> {
    let img = image::load_from_memory(png_bytes)?;
    let rgb = img.to_rgb8();
    let (w, h) = (rgb.width(), rgb.height());
    let compressed = miniz_oxide::deflate::compress_to_vec_zlib(&rgb.into_raw(), 6);

    let mut pdf = Vec::new();
    pdf.extend_from_slice(b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n");

    let mut offsets = Vec::new();

    offsets.push(pdf.len());
    pdf.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");

    offsets.push(pdf.len());
    pdf.extend_from_slice(b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n");

    offsets.push(pdf.len());
    let page = format!("3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {} {}] /Contents 4 0 R /Resources << /XObject << /Im0 5 0 R >> >> >>\nendobj\n", w, h);
    pdf.extend_from_slice(page.as_bytes());

    offsets.push(pdf.len());
    let stream = format!("q\n{} 0 0 {} 0 0 cm\n/Im0 Do\nQ\n", w, h);
    let contents = format!("4 0 obj\n<< /Length {} >>\nstream\n{}endstream\nendobj\n", stream.len(), stream);
    pdf.extend_from_slice(contents.as_bytes());

    offsets.push(pdf.len());
    let img_hdr = format!("5 0 obj\n<< /Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /FlateDecode /Length {} >>\nstream\n", w, h, compressed.len());
    pdf.extend_from_slice(img_hdr.as_bytes());
    pdf.extend_from_slice(&compressed);
    pdf.extend_from_slice(b"\nendstream\nendobj\n");

    let xref_offset = pdf.len();
    pdf.extend_from_slice(format!("xref\n0 {}\n", offsets.len() + 1).as_bytes());
    pdf.extend_from_slice(b"0000000000 65535 f \n");
    for &off in &offsets {
        pdf.extend_from_slice(format!("{:010} 00000 n \n", off).as_bytes());
    }

    let trailer = format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{}\n%%EOF\n", offsets.len() + 1, xref_offset);
    pdf.extend_from_slice(trailer.as_bytes());

    Ok(pdf)
}

fn crop_for_card(img: DynamicImage, fx: f32, fy: f32) -> DynamicImage {
    let (w, h) = img.dimensions();
    let (x, y, cw, ch) = crop_bounds(w, h, fx, fy);
    img.crop_imm(x, y, cw, ch)
}

fn crop_bounds(w: u32, h: u32, fx: f32, fy: f32) -> (u32, u32, u32, u32) {
    let card_ratio = CARD_WIDTH as f64 / CARD_HEIGHT as f64;
    let src_ratio = w as f64 / h as f64;

    if src_ratio > card_ratio {
        let cw = ((h as f64 * card_ratio).round() as u32).clamp(1, w);
        let max_x = w.saturating_sub(cw);
        let x = (fx.clamp(0.0, 1.0) * max_x as f32).round() as u32;
        (x, 0, cw, h)
    } else {
        let ch = ((w as f64 / card_ratio).round() as u32).clamp(1, h);
        let max_y = h.saturating_sub(ch);
        let y = (fy.clamp(0.0, 1.0) * max_y as f32).round() as u32;
        (0, y, w, ch)
    }
}
