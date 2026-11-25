//! The small JSON objects the page reads (no serde: a handful of fields).

use orpheus_host::flows::{ChipKind, Slots};
use orpheus_proto::devchan::Info;
use orpheus_update::ofw::{self, Header};
use orpheus_update::slots::SlotState;

/// `"…"` with JSON escapes.
#[must_use]
pub fn string(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

#[must_use]
pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// An update file's header: version, build id, image size, change list.
#[must_use]
pub fn ofw(h: &Header, header_bytes: &[u8]) -> String {
    format!(
        "{{\"version\":{},\"build\":{},\"size\":{},\"layout\":{},\"changes\":{}}}",
        string(h.version_str().unwrap_or("?")),
        string(&hex(&h.build_id)),
        h.image_len,
        h.layout,
        string(ofw::changes(header_bytes))
    )
}

/// The developer channel's INFO.
#[must_use]
pub fn info(i: &Info) -> String {
    format!(
        "{{\"version\":{},\"layout\":{},\"personality\":{},\"uptime_ms\":{},\"battery_mv\":{},\"build\":{}}}",
        i.version,
        i.layout,
        i.personality,
        i.uptime_ms,
        i.battery_mv,
        string(&hex(&i.build_id))
    )
}

fn slot(s: SlotState) -> String {
    match s {
        SlotState::Valid(b) => format!("{{\"state\":\"valid\",\"build\":{}}}", string(&hex(&b))),
        SlotState::Superseded(b) => format!("{{\"state\":\"superseded\",\"build\":{}}}", string(&hex(&b))),
        SlotState::Empty => "{\"state\":\"empty\"}".into(),
    }
}

/// Recovery's view of the two slots.
#[must_use]
pub fn slots(s: &Slots) -> String {
    format!("{{\"a\":{},\"b\":{},\"can_switch\":{}}}", slot(s.a), slot(s.b), s.can_switch)
}

#[must_use]
pub fn chip_kind(k: ChipKind) -> &'static str {
    match k {
        ChipKind::Stock => "stock",
        ChipKind::OrpheusFull => "orpheus",
        ChipKind::Other => "other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_and_objects() {
        assert_eq!(string("a\"b\\c\nd\u{1}é"), "\"a\\\"b\\\\c\\nd\\u0001é\"");
        let img = orpheus_update::sim::image(5, 0x1000, 1);
        let hb = orpheus_update::sim::ofw_header(&img, "1.2.3", "One \"fix\"\nTwo");
        let h = Header::parse(&hb).unwrap();
        let j = ofw(&h, &hb);
        assert!(j.starts_with("{\"version\":\"1.2.3\",\"build\":\"05"), "{j}");
        assert!(j.ends_with("\"changes\":\"One \\\"fix\\\"\\nTwo\"}"), "{j}");
        let i = Info { version: 1, build_id: [0xAB; 16], ..Info::default() };
        assert!(info(&i).contains("\"build\":\"abababab"));
        let s = Slots { a: SlotState::Superseded([1; 16]), b: SlotState::Valid([2; 16]), can_switch: true };
        assert!(slots(&s).contains("\"a\":{\"state\":\"superseded\""));
        assert_eq!(chip_kind(ChipKind::Stock), "stock");
    }
}
