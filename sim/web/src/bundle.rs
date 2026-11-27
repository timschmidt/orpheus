//! The demo card's files as one download (WEB-2): `cargo xtask wasm` packs
//! `sim/web/demo/card/` into `try/pkg/demo.bin`; the page fetches it next
//! to the module and hands it to [`crate::Web::add_demo`] before the boot.
//! Keeping the media out of the module keeps the module small (its first
//! frame does not wait for megabytes of music) and lets the browser cache
//! the two separately.
//!
//! Format (little-endian): `ODC1`, the entry count (u32), then per entry
//! the path length (u16), the path (UTF-8, `/`-separated, leading `/`),
//! the method (u8: 0 stored, 1 raw deflate), the length of the file (u32)
//! and of its stored bytes (u32), the stored bytes. Text files are stored
//! deflated (GitHub Pages does not compress an `application/octet-stream`
//! download); media already compressed are stored as they are.
//!
//! The input is checked: lengths are bounded and a deflated entry must
//! inflate to exactly its stated length.

use crate::card::File;

const MAGIC: &[u8; 4] = b"ODC1";
/// The largest file in a bundle (the demo's are a few hundred KB).
pub const MAX_FILE: usize = 32 << 20;
/// The most entries.
pub const MAX_ENTRIES: usize = 4096;

/// Pack files (paths with a leading `/`) in the given order.
#[must_use]
pub fn encode(files: &[File]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&(files.len() as u32).to_le_bytes());
    for f in files {
        let packed = miniz_oxide::deflate::compress_to_vec(&f.data, 9);
        // Deflate only what it shrinks by a tenth or more.
        let (method, body) =
            if packed.len() * 10 < f.data.len() * 9 { (1u8, &packed[..]) } else { (0, &f.data[..]) };
        out.extend_from_slice(&(f.path.len() as u16).to_le_bytes());
        out.extend_from_slice(f.path.as_bytes());
        out.push(method);
        out.extend_from_slice(&(f.data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(body);
    }
    out
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        if self.0.len() < n {
            return Err("the demo bundle is truncated".into());
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Ok(a)
    }
    fn u16(&mut self) -> Result<usize, String> {
        Ok(usize::from(u16::from_le_bytes([self.take(1)?[0], self.take(1)?[0]])))
    }
    fn u32(&mut self) -> Result<usize, String> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
    }
}

/// Unpack a bundle.
pub fn decode(data: &[u8]) -> Result<Vec<File>, String> {
    let mut r = Reader(data);
    if r.take(4)? != MAGIC {
        return Err("not a demo bundle".into());
    }
    let n = r.u32()?;
    if n > MAX_ENTRIES {
        return Err(format!("{n} entries in the demo bundle"));
    }
    let mut files = Vec::with_capacity(n);
    for _ in 0..n {
        let len = r.u16()?;
        let path = std::str::from_utf8(r.take(len)?).map_err(|_| "a path is not UTF-8".to_string())?;
        if !path.starts_with('/') || path.split('/').skip(1).any(|c| c.is_empty() || c == "." || c == "..") {
            return Err(format!("bad path {path:?} in the demo bundle"));
        }
        let method = r.take(1)?[0];
        let size = r.u32()?;
        let stored = r.u32()?;
        if size > MAX_FILE || stored > MAX_FILE {
            return Err(format!("{path}: too large"));
        }
        let body = r.take(stored)?;
        let data = match method {
            0 if stored == size => body.to_vec(),
            1 => miniz_oxide::inflate::decompress_to_vec_with_limit(body, size)
                .ok()
                .filter(|d| d.len() == size)
                .ok_or_else(|| format!("{path}: does not inflate to {size} bytes"))?,
            _ => return Err(format!("{path}: bad entry")),
        };
        files.push(File { path: path.to_string(), data });
    }
    if !r.0.is_empty() {
        return Err("trailing bytes after the demo bundle".into());
    }
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(path: &str, data: Vec<u8>) -> File {
        File { path: path.into(), data }
    }

    #[test]
    fn round_trip_stores_media_and_deflates_text() {
        let text = b"The Pied Piper of Hamelin\n".repeat(200);
        let noise: Vec<u8> = (0..5000u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 24) as u8).collect();
        let files =
            vec![f("/Books/a.txt", text.clone()), f("/Music/b.mp3", noise.clone()), f("/empty", vec![])];
        let b = encode(&files);
        assert!(b.len() < text.len() / 4 + noise.len() + 200, "{}", b.len());
        let back = decode(&b).unwrap();
        assert_eq!(back.len(), 3);
        for (a, b) in files.iter().zip(&back) {
            assert_eq!(a.path, b.path);
            assert!(a.data == b.data, "{}", a.path);
        }
    }

    #[test]
    fn hostile_bundles_are_refused() {
        let good = encode(&[f("/a.txt", b"hello hello hello hello hello hello".to_vec())]);
        assert!(decode(&good).is_ok());
        for cut in 0..good.len() {
            assert!(decode(&good[..cut]).is_err(), "truncated at {cut}");
        }
        let mut extra = good.clone();
        extra.push(0);
        assert!(decode(&extra).is_err());
        for bad in ["a.txt", "/../a", "/x//y", "/./a"] {
            assert!(decode(&encode(&[f(bad, vec![1])])).is_err(), "{bad}");
        }
        // A deflated entry that claims another length.
        let mut lie = good.clone();
        let at = 4 + 4 + 2 + "/a.txt".len() + 1;
        lie[at] ^= 1;
        assert!(decode(&lie).is_err());
        assert!(decode(b"ODC1\xff\xff\xff\xff").is_err());
    }
}
