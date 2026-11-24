//! Prepared list rows: labels borrowed from the string table, values
//! formatted into inline buffers. List screens rebuild them when something
//! changes and hand them to `ListView` as its `ListSource`.

use yoton_ui::icons::Icon;
use yoton_ui::text::StrBuf;
use yoton_ui::widgets::list::{Accessory, Item, ListSource, Row};

/// Most rows a list screen shows (header + the longest list: 12 groups or 11 presets).
pub const MAX_ROWS: usize = 13;

#[derive(Clone, Copy, Debug)]
pub struct RowData {
    pub label: &'static str,
    pub value: StrBuf<24>,
    pub acc: Accessory,
    pub icon: Option<&'static Icon>,
    pub header: bool,
    pub dim: bool,
    pub wide_value: bool,
    /// What the row is (a setting key, an action code …), for activation.
    pub tag: u16,
}

impl RowData {
    const EMPTY: RowData = RowData {
        label: "",
        value: StrBuf::new(),
        acc: Accessory::None,
        icon: None,
        header: false,
        dim: false,
        wide_value: false,
        tag: 0,
    };
}

#[derive(Clone, Copy, Debug)]
pub struct Rows {
    rows: [RowData; MAX_ROWS],
    n: u8,
}

impl Default for Rows {
    fn default() -> Self {
        Rows { rows: [RowData::EMPTY; MAX_ROWS], n: 0 }
    }
}

impl Rows {
    pub fn clear(&mut self) {
        self.n = 0;
    }

    /// Append a row; returns it for further setup (`None` when full).
    pub fn push(&mut self, label: &'static str, tag: u16) -> Option<&mut RowData> {
        let r = self.rows.get_mut(usize::from(self.n))?;
        *r = RowData { label, tag, ..RowData::EMPTY };
        self.n += 1;
        Some(r)
    }

    #[must_use]
    pub fn get(&self, i: u32) -> Option<&RowData> {
        usize::try_from(i).ok().filter(|&i| i < usize::from(self.n)).and_then(|i| self.rows.get(i))
    }

    pub fn get_mut(&mut self, i: u32) -> Option<&mut RowData> {
        let n = usize::from(self.n);
        usize::try_from(i).ok().filter(|&i| i < n).and_then(|i| self.rows.get_mut(i))
    }

    /// Index of the row with `tag`.
    #[must_use]
    pub fn find(&self, tag: u16) -> Option<u32> {
        self.rows[..usize::from(self.n)]
            .iter()
            .position(|r| r.tag == tag && !r.header)
            .and_then(|i| u32::try_from(i).ok())
    }
}

impl ListSource for Rows {
    fn len(&self) -> u32 {
        u32::from(self.n)
    }

    fn row(&self, i: u32) -> Row<'_> {
        let Some(r) = self.get(i) else { return Row::Pending };
        if r.header {
            return Row::Header(r.label);
        }
        // A row without a label shows its formatted value as the text.
        let mut it = if r.label.is_empty() {
            Item::new(r.value.as_str()).accessory(r.acc)
        } else {
            let it = Item::new(r.label).accessory(r.acc);
            if r.value.is_empty() { it } else { it.value(r.value.as_str()) }
        };
        if let Some(ic) = r.icon {
            it = it.icon(ic);
        }
        if r.dim {
            it = it.dim();
        }
        if r.wide_value {
            it = it.wide_value();
        }
        Row::Item(it)
    }
}
