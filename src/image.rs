//! Small PNG helpers for clipboard images: validate untrusted uploads, convert to and from RGBA.
//!
//! Phones send PNG only (the page converts photos with a canvas), so no other decoder is needed.
//! Dimensions are checked before any pixel data is decoded, so a tiny file that claims a huge
//! image cannot make the server allocate gigabytes.

use std::io::Cursor;

/// Longest allowed side in pixels.
pub const MAX_SIDE: u32 = 8192;
/// Largest allowed pixel count (36 MP is about 144 MB as RGBA).
pub const MAX_PIXELS: u64 = 36_000_000;
/// Largest accepted upload.
pub const MAX_PNG_BYTES: usize = 32 * 1024 * 1024;

const SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";

/// Width and height of a PNG without decoding it. Errors are short, client-safe messages.
pub fn png_dimensions(data: &[u8]) -> Result<(u32, u32), &'static str> {
    if data.len() < 8 || &data[..8] != SIGNATURE {
        return Err("not a PNG image");
    }
    let reader = png::Decoder::new(Cursor::new(data)).read_info().map_err(|_| "invalid PNG")?;
    let info = reader.info();
    let (w, h) = (info.width, info.height);
    if w == 0 || h == 0 {
        return Err("empty image");
    }
    if w > MAX_SIDE || h > MAX_SIDE || u64::from(w) * u64::from(h) > MAX_PIXELS {
        return Err("image too large");
    }
    Ok((w, h))
}

/// Decode any PNG (palette, gray, 16-bit, with or without alpha) to 8-bit RGBA.
pub fn decode_rgba(data: &[u8]) -> Result<(u32, u32, Vec<u8>), &'static str> {
    png_dimensions(data)?;
    let mut decoder = png::Decoder::new(Cursor::new(data));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(|_| "invalid PNG")?;
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let frame = reader.next_frame(&mut buf).map_err(|_| "invalid PNG data")?;
    let px = &buf[..frame.buffer_size()];
    let rgba = match frame.color_type {
        png::ColorType::Rgba => px.to_vec(),
        png::ColorType::Rgb => px.chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        png::ColorType::GrayscaleAlpha => px.chunks_exact(2).flat_map(|p| [p[0], p[0], p[0], p[1]]).collect(),
        png::ColorType::Grayscale => px.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => return Err("unsupported PNG"),
    };
    Ok((frame.width, frame.height, rgba))
}

/// Encode 8-bit RGBA pixels as a PNG.
pub fn encode_rgba(width: u32, height: u32, rgba: &[u8]) -> Option<Vec<u8>> {
    if width == 0 || height == 0 || rgba.len() != width as usize * height as usize * 4 {
        return None;
    }
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, width, height);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header().ok()?.write_image_data(rgba).ok()?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(w: u32, h: u32) -> Vec<u8> {
        (0..w * h).flat_map(|i| [(i % 251) as u8, (i % 13) as u8 * 19, 200, if i % 5 == 0 { 128 } else { 255 }]).collect()
    }

    #[test]
    fn rgba_roundtrip_is_lossless() {
        let px = sample(7, 5);
        let png = encode_rgba(7, 5, &px).unwrap();
        assert_eq!(png_dimensions(&png), Ok((7, 5)));
        assert_eq!(decode_rgba(&png).unwrap(), (7, 5, px));
    }

    #[test]
    fn rejects_garbage_and_truncated_data() {
        assert_eq!(png_dimensions(b""), Err("not a PNG image"));
        assert_eq!(png_dimensions(b"GIF89a....................."), Err("not a PNG image"));
        let png = encode_rgba(4, 4, &sample(4, 4)).unwrap();
        assert!(decode_rgba(&png[..png.len() / 2]).is_err());
        assert_eq!(png_dimensions(&png[..20]), Err("invalid PNG"));
    }

    #[test]
    fn rejects_oversized_dimensions_without_decoding() {
        // a real 1-bit PNG of 8200 x 8200 pixels: about 8 MB of zeros that compress to a small file
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut out, 8_200, 8_200);
            enc.set_color(png::ColorType::Grayscale);
            enc.set_depth(png::BitDepth::One);
            let mut w = enc.write_header().unwrap();
            w.write_image_data(&vec![0u8; 1025 * 8_200]).unwrap();
        }
        assert!(out.len() < 1_000_000, "the file is small even though the claimed image is huge: {}", out.len());
        assert_eq!(png_dimensions(&out), Err("image too large"));
        assert_eq!(decode_rgba(&out).unwrap_err(), "image too large");
    }

    #[test]
    fn converts_gray_and_rgb_to_rgba() {
        let mut gray = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut gray, 2, 1);
            enc.set_color(png::ColorType::Grayscale);
            enc.set_depth(png::BitDepth::Eight);
            enc.write_header().unwrap().write_image_data(&[10, 250]).unwrap();
        }
        assert_eq!(decode_rgba(&gray).unwrap().2, vec![10, 10, 10, 255, 250, 250, 250, 255]);

        let mut rgb = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut rgb, 1, 1);
            enc.set_color(png::ColorType::Rgb);
            enc.set_depth(png::BitDepth::Eight);
            enc.write_header().unwrap().write_image_data(&[1, 2, 3]).unwrap();
        }
        assert_eq!(decode_rgba(&rgb).unwrap().2, vec![1, 2, 3, 255]);
    }

    #[test]
    fn encode_rejects_wrong_buffer_size() {
        assert!(encode_rgba(2, 2, &[0; 15]).is_none());
        assert!(encode_rgba(0, 2, &[]).is_none());
    }
}
