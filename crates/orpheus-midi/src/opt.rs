//! `Opt<T>`: an `Option<T>` whose all-zero value is valid (`None`), so the
//! player stays `bytemuck::Zeroable` and can be built in place in the
//! decoder arena (SYN-3; `Option<T>` has no defined zero value for most
//! `T`).

/// `Some(v)` when `some`; `v` is meaningless otherwise.
#[derive(bytemuck::Zeroable, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Opt<T> {
    some: bool,
    v: T,
}

impl<T: Copy + bytemuck::Zeroable> Opt<T> {
    /// The value.
    pub(crate) fn get(&self) -> Option<T> {
        self.some.then_some(self.v)
    }

    /// A reference to the value.
    pub(crate) fn as_ref(&self) -> Option<&T> {
        self.some.then_some(&self.v)
    }

    /// Set (or clear) the value. `None` zeroes the value too: two `None`s
    /// are the same bytes (and compare and print the same).
    pub(crate) fn set(&mut self, v: Option<T>) {
        self.some = v.is_some();
        self.v = v.unwrap_or_else(T::zeroed);
    }

    /// The value, leaving `None`.
    pub(crate) fn take(&mut self) -> Option<T> {
        let v = self.get();
        self.set(None);
        v
    }
}

#[cfg(test)]
mod tests {
    use super::Opt;

    #[test]
    fn zero_is_none_and_round_trips() {
        let mut o = <Opt<(u64, u8)> as bytemuck::Zeroable>::zeroed();
        assert_eq!(o.get(), None);
        o.set(Some((7, 3)));
        assert_eq!(o.as_ref(), Some(&(7, 3)));
        assert_eq!(o.take(), Some((7, 3)));
        assert_eq!(o.get(), None);
    }
}
