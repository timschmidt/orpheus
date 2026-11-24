//! RGB565 frame buffers to PNG and back.

use std::path::Path;

use anyhow::{Context, Result, bail};

/// Expand one little-endian RGB565 pixel to RGB888 (bit replication).
#[must_use]
pub fn rgb565_to_rgb888(v: u16) -> [u8; 3] {
    let r = ((v >> 11) & 31) as u8;
    let g = ((v >> 5) & 63) as u8;
    let b = (v & 31) as u8;
    [(r << 3) | (r >> 2), (g << 2) | (g >> 4), (b << 3) | (b >> 2)]
}

/// Encode an RGB565 (little-endian) buffer as PNG bytes.
pub fn rgb565_png_bytes(data: &[u8], w: u32, h: u32) -> Result<Vec<u8>> {
    let n = (w * h) as usize;
    if data.len() < n * 2 {
        bail!("frame buffer has {} bytes, need {}", data.len(), n * 2);
    }
    let mut rgb = Vec::with_capacity(n * 3);
    for px in data[..n * 2].as_chunks::<2>().0 {
        rgb.extend_from_slice(&rgb565_to_rgb888(u16::from_le_bytes([px[0], px[1]])));
    }
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        let mut wr = enc.write_header()?;
        wr.write_image_data(&rgb)?;
    }
    Ok(out)
}

pub fn write_rgb565_png(data: &[u8], w: u32, h: u32, path: &Path) -> Result<()> {
    let bytes = rgb565_png_bytes(data, w, h)?;
    std::fs::write(path, bytes).with_context(|| format!("write {}", path.display()))
}

/// Decode a PNG to RGB888 rows (for golden comparisons).
pub fn read_png_rgb(path: &Path) -> Result<(u32, u32, Vec<u8>)> {
    let f = std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut dec = png::Decoder::new(std::io::BufReader::new(f));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut r = dec.read_info()?;
    let mut buf = vec![0; r.output_buffer_size().unwrap_or(0)];
    let info = r.next_frame(&mut buf)?;
    buf.truncate(info.buffer_size());
    let rgb = match info.color_type {
        png::ColorType::Rgb => buf,
        png::ColorType::Rgba => buf.as_chunks::<4>().0.iter().flat_map(|p| [p[0], p[1], p[2]]).collect(),
        png::ColorType::Grayscale => buf.iter().flat_map(|&g| [g, g, g]).collect(),
        png::ColorType::GrayscaleAlpha => {
            buf.as_chunks::<2>().0.iter().flat_map(|p| [p[0], p[0], p[0]]).collect()
        }
        png::ColorType::Indexed => bail!("indexed PNG not expanded"),
    };
    Ok((info.width, info.height, rgb))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand() {
        assert_eq!(rgb565_to_rgb888(0xFFFF), [255, 255, 255]);
        assert_eq!(rgb565_to_rgb888(0xF800), [255, 0, 0]);
        assert_eq!(rgb565_to_rgb888(0x07E0), [0, 255, 0]);
        assert_eq!(rgb565_to_rgb888(0x001F), [0, 0, 255]);
    }

    #[test]
    fn png_roundtrip() {
        let mut fb = vec![0u8; 4 * 4 * 2];
        fb[0..2].copy_from_slice(&0xF800u16.to_le_bytes());
        let bytes = rgb565_png_bytes(&fb, 4, 4).unwrap();
        let dir = std::env::temp_dir().join(format!("orpheus-host-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("x.png");
        std::fs::write(&p, bytes).unwrap();
        let (w, h, rgb) = read_png_rgb(&p).unwrap();
        assert_eq!((w, h), (4, 4));
        assert_eq!(&rgb[..3], &[255, 0, 0]);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
