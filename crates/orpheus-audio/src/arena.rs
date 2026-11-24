//! The codec arena: a bump allocator over a lease from the mode pool
//! (02 §1.3, 04 §4.1).
//!
//! A decoder takes every buffer it needs from its arena when it opens; it
//! never allocates afterwards and never puts large buffers on the stack.
//! Allocation is bump-only, so the bytes handed out so far are the high-water
//! mark: [`Arena::used`]. An allocation that does not fit returns
//! [`ArenaFull`] with the total that would have been needed, and the decoder
//! refuses the stream cleanly.
//!
//! Typed buffers come from the byte slice through `bytemuck` (`Pod` types
//! only: integers, floats, arrays of them and `#[repr(C)]` structs without
//! padding), so no `unsafe` is needed here or in the decoders.

use bytemuck::Pod;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArenaFull {
    /// Bytes the arena would have needed to satisfy this allocation.
    pub needed: usize,
    /// Bytes the arena has.
    pub capacity: usize,
}

#[derive(Debug)]
pub struct Arena<'a> {
    rest: &'a mut [u8],
    used: usize,
    capacity: usize,
}

impl<'a> Arena<'a> {
    #[must_use]
    pub fn new(buf: &'a mut [u8]) -> Self {
        let capacity = buf.len();
        Self { rest: buf, used: 0, capacity }
    }

    /// Bytes handed out so far, including alignment padding (the high-water
    /// mark: nothing is ever returned).
    #[must_use]
    pub fn used(&self) -> usize {
        self.used
    }

    #[must_use]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    #[must_use]
    pub fn remaining(&self) -> usize {
        self.rest.len()
    }

    /// Take `size` bytes aligned to `align` (a power of two), zeroed.
    fn take(&mut self, size: usize, align: usize) -> Result<&'a mut [u8], ArenaFull> {
        let addr = self.rest.as_ptr() as usize;
        let pad = addr.wrapping_neg() & (align - 1);
        let total = pad.checked_add(size).ok_or(ArenaFull { needed: usize::MAX, capacity: self.capacity })?;
        if total > self.rest.len() {
            return Err(ArenaFull { needed: self.used.saturating_add(total), capacity: self.capacity });
        }
        let buf = core::mem::take(&mut self.rest);
        let (head, tail) = buf.split_at_mut(total);
        self.rest = tail;
        self.used += total;
        let out = &mut head[pad..];
        out.fill(0);
        Ok(out)
    }

    /// A zeroed slice of `n` values.
    pub fn slice<T: Pod>(&mut self, n: usize) -> Result<&'a mut [T], ArenaFull> {
        let size = core::mem::size_of::<T>()
            .checked_mul(n)
            .ok_or(ArenaFull { needed: usize::MAX, capacity: self.capacity })?;
        let bytes = self.take(size, core::mem::align_of::<T>())?;
        if size == 0 {
            return Ok(&mut []);
        }
        // Size and alignment were arranged above, so the cast cannot fail.
        bytemuck::try_cast_slice_mut(bytes).map_err(|_| ArenaFull { needed: 0, capacity: self.capacity })
    }

    /// One zeroed value (a `#[repr(C)]` state struct, an array).
    pub fn one<T: Pod>(&mut self) -> Result<&'a mut T, ArenaFull> {
        let bytes = self.take(core::mem::size_of::<T>(), core::mem::align_of::<T>())?;
        bytemuck::try_from_bytes_mut(bytes).map_err(|_| ArenaFull { needed: 0, capacity: self.capacity })
    }
}

/// Conservative arena bytes for `n` values of `T` (size plus worst-case
/// alignment padding). Decoders add these up to state their need before
/// allocating.
#[must_use]
pub const fn need<T>(n: usize) -> usize {
    n * core::mem::size_of::<T>() + core::mem::align_of::<T>() - 1
}

/// Reinterpret an `i32` buffer as `i16`s (twice as many). Decoders keep one
/// output buffer typed `i32` and hand out either width from it.
#[must_use]
pub fn as_i16(buf: &mut [i32]) -> &mut [i16] {
    bytemuck::cast_slice_mut(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy, bytemuck::Zeroable, bytemuck::Pod)]
    #[repr(C)]
    struct S {
        a: [f32; 3],
        b: u32,
    }

    #[test]
    fn bump_and_exhaust() {
        let mut mem = [0xA5u8; 64];
        let mut a = Arena::new(&mut mem);
        let x = a.slice::<u8>(3).unwrap();
        assert_eq!(x, &[0, 0, 0]);
        let s = a.one::<S>().unwrap();
        assert_eq!(s.b, 0);
        s.a[2] = 1.5;
        let used = a.used();
        assert!((3 + 16..=3 + 16 + 3).contains(&used));
        let e = a.slice::<u32>(100).unwrap_err();
        assert_eq!(e.capacity, 64);
        assert!(e.needed > 64);
        assert_eq!(a.used(), used);
        let rest = a.remaining();
        let r = a.slice::<u8>(rest).unwrap();
        assert_eq!(r.len(), rest);
        assert_eq!(a.remaining(), 0);
        assert!(a.slice::<u8>(1).is_err());
    }

    #[test]
    fn i16_view() {
        let mut b = [0i32; 4];
        let h = as_i16(&mut b);
        assert_eq!(h.len(), 8);
        h[1] = -1;
        assert_ne!(b[0], 0);
    }
}
