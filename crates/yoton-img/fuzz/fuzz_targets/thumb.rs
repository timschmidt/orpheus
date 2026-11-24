//! Thumbnail cache lookups and stores on arbitrary bucket-file contents
//! never panic, and a store followed by a lookup finds the thumbnail.
#![no_main]

use libfuzzer_sys::fuzz_target;
use yoton_img::thumb::{CacheFile, THUMBS, ThumbKey, ThumbMeta};

struct Mem(Vec<u8>);

impl CacheFile for Mem {
    type Error = ();
    fn read_at(&mut self, off: u32, buf: &mut [u8]) -> Result<usize, ()> {
        let off = (off as usize).min(self.0.len());
        let n = buf.len().min(self.0.len() - off);
        buf[..n].copy_from_slice(&self.0[off..off + n]);
        Ok(n)
    }
    fn write_at(&mut self, off: u32, data: &[u8]) -> Result<(), ()> {
        let end = off as usize + data.len();
        if end > 1 << 22 {
            return Err(());
        }
        if self.0.len() < end {
            self.0.resize(end, 0);
        }
        self.0[off as usize..end].copy_from_slice(data);
        Ok(())
    }
}

fuzz_target!(|data: &[u8]| {
    let kind = THUMBS;
    let mut f = Mem(data.to_vec());
    let key = ThumbKey { file_size: data.len() as u64, w: kind.w, h: kind.h, ..Default::default() };
    let mut out = vec![0u16; usize::from(kind.w) * usize::from(kind.h)];
    let _ = kind.lookup(&mut f, &key, &mut out);
    let px: Vec<u16> = (0..out.len()).map(|i| i as u16).collect();
    let meta = ThumbMeta { orig_w: 1, orig_h: 2, approximate: false };
    if kind.store(&mut f, &key, &meta, &px).is_ok() {
        assert_eq!(kind.lookup(&mut f, &key, &mut out), Ok(Some(meta)));
        assert_eq!(out, px);
    }
});
