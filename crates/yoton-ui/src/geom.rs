//! Integer geometry in screen pixels.

/// A rectangle; `w` and `h` are never negative.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Rect {
    pub x: i16,
    pub y: i16,
    pub w: i16,
    pub h: i16,
}

impl Rect {
    pub const EMPTY: Rect = Rect { x: 0, y: 0, w: 0, h: 0 };

    #[must_use]
    pub const fn new(x: i16, y: i16, w: i16, h: i16) -> Rect {
        Rect { x, y, w: if w < 0 { 0 } else { w }, h: if h < 0 { 0 } else { h } }
    }

    /// Rectangle from inclusive-exclusive corners.
    #[must_use]
    pub const fn from_edges(x0: i16, y0: i16, x1: i16, y1: i16) -> Rect {
        Rect::new(x0, y0, x1 - x0, y1 - y0)
    }

    #[must_use]
    pub const fn right(&self) -> i16 {
        self.x + self.w
    }

    #[must_use]
    pub const fn bottom(&self) -> i16 {
        self.y + self.h
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.w <= 0 || self.h <= 0
    }

    #[must_use]
    pub fn area(&self) -> i32 {
        i32::from(self.w) * i32::from(self.h)
    }

    #[must_use]
    pub fn intersect(&self, o: &Rect) -> Rect {
        let x0 = self.x.max(o.x);
        let y0 = self.y.max(o.y);
        let x1 = self.right().min(o.right());
        let y1 = self.bottom().min(o.bottom());
        if x1 <= x0 || y1 <= y0 { Rect::EMPTY } else { Rect::from_edges(x0, y0, x1, y1) }
    }

    /// Smallest rectangle containing both (an empty operand is ignored).
    #[must_use]
    pub fn union(&self, o: &Rect) -> Rect {
        if self.is_empty() {
            return *o;
        }
        if o.is_empty() {
            return *self;
        }
        Rect::from_edges(
            self.x.min(o.x),
            self.y.min(o.y),
            self.right().max(o.right()),
            self.bottom().max(o.bottom()),
        )
    }

    #[must_use]
    pub fn contains_rect(&self, o: &Rect) -> bool {
        o.x >= self.x && o.y >= self.y && o.right() <= self.right() && o.bottom() <= self.bottom()
    }

    #[must_use]
    pub fn contains(&self, x: i16, y: i16) -> bool {
        x >= self.x && y >= self.y && x < self.right() && y < self.bottom()
    }

    /// True if the rectangles overlap or share an edge.
    #[must_use]
    pub fn touches(&self, o: &Rect) -> bool {
        self.x <= o.right() && o.x <= self.right() && self.y <= o.bottom() && o.y <= self.bottom()
    }

    #[must_use]
    pub const fn translate(&self, dx: i16, dy: i16) -> Rect {
        Rect { x: self.x + dx, y: self.y + dy, w: self.w, h: self.h }
    }

    /// Shrink by `dx` on the left and right and `dy` on the top and bottom.
    #[must_use]
    pub const fn inset(&self, dx: i16, dy: i16) -> Rect {
        Rect::new(self.x + dx, self.y + dy, self.w - 2 * dx, self.h - 2 * dy)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intersect_union() {
        let a = Rect::new(0, 0, 10, 10);
        let b = Rect::new(5, 5, 10, 10);
        assert_eq!(a.intersect(&b), Rect::new(5, 5, 5, 5));
        assert_eq!(a.union(&b), Rect::new(0, 0, 15, 15));
        assert!(a.intersect(&Rect::new(20, 20, 1, 1)).is_empty());
        assert_eq!(Rect::EMPTY.union(&b), b);
        assert!(a.touches(&Rect::new(10, 0, 2, 2)));
        assert!(!a.touches(&Rect::new(11, 0, 2, 2)));
    }
}
