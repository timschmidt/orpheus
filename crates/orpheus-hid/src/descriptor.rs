//! Report descriptors.

/// HID class code of an interface; subclass 1 = boot interface; protocol
/// 1 = keyboard.
pub const CLASS_HID: u8 = 0x03;
pub const SUBCLASS_BOOT: u8 = 0x01;
pub const PROTOCOL_KEYBOARD: u8 = 0x01;

/// The boot keyboard of HID 1.11 Appendix B.1 (also the report protocol's
/// descriptor, so the same 8-byte report works in both protocols, in PC
/// firmware set-up screens and in full-disk-encryption prompts):
/// 8 modifier bits, a reserved byte, 5 LED bits out (+ 3 padding), 6 key
/// slots over usages 0..=101.
pub const BOOT_KEYBOARD: [u8; 63] = [
    0x05, 0x01, // Usage Page (Generic Desktop)
    0x09, 0x06, // Usage (Keyboard)
    0xA1, 0x01, // Collection (Application)
    0x05, 0x07, //   Usage Page (Keyboard/Keypad)
    0x19, 0xE0, //   Usage Minimum (Left Control)
    0x29, 0xE7, //   Usage Maximum (Right GUI)
    0x15, 0x00, //   Logical Minimum (0)
    0x25, 0x01, //   Logical Maximum (1)
    0x75, 0x01, //   Report Size (1)
    0x95, 0x08, //   Report Count (8)
    0x81, 0x02, //   Input (Data, Variable, Absolute): modifiers
    0x95, 0x01, //   Report Count (1)
    0x75, 0x08, //   Report Size (8)
    0x81, 0x01, //   Input (Constant): reserved byte
    0x95, 0x05, //   Report Count (5)
    0x75, 0x01, //   Report Size (1)
    0x05, 0x08, //   Usage Page (LEDs)
    0x19, 0x01, //   Usage Minimum (Num Lock)
    0x29, 0x05, //   Usage Maximum (Kana)
    0x91, 0x02, //   Output (Data, Variable, Absolute): LEDs
    0x95, 0x01, //   Report Count (1)
    0x75, 0x03, //   Report Size (3)
    0x91, 0x01, //   Output (Constant): padding
    0x95, 0x06, //   Report Count (6)
    0x75, 0x08, //   Report Size (8)
    0x15, 0x00, //   Logical Minimum (0)
    0x25, 0x65, //   Logical Maximum (101)
    0x05, 0x07, //   Usage Page (Keyboard/Keypad)
    0x19, 0x00, //   Usage Minimum (0)
    0x29, 0x65, //   Usage Maximum (101)
    0x81, 0x00, //   Input (Data, Array): key slots
    0xC0, // End Collection
];

/// Input report length of [`BOOT_KEYBOARD`].
pub const BOOT_KEYBOARD_REPORT: usize = 8;

#[cfg(test)]
mod tests {
    use super::*;

    /// Walk the short items: every item is complete, collections balance,
    /// and the input items add up to 8 bytes, the output items to 1.
    #[test]
    fn boot_keyboard_is_well_formed() {
        let d = &BOOT_KEYBOARD;
        let (mut i, mut depth) = (0, 0i32);
        let (mut size, mut count) = (0u32, 0u32);
        let (mut in_bits, mut out_bits) = (0u32, 0u32);
        while i < d.len() {
            let prefix = d[i];
            let n = match prefix & 3 {
                3 => 4,
                k => usize::from(k),
            };
            assert!(i + n < d.len(), "item at {i} runs past the end");
            let v = if n == 0 { 0 } else { u32::from(d[i + 1]) };
            match prefix & 0xFC {
                0xA0 => depth += 1,
                0xC0 => depth -= 1,
                0x74 => size = v,
                0x94 => count = v,
                0x80 => in_bits += size * count,
                0x90 => out_bits += size * count,
                _ => {}
            }
            i += 1 + n;
        }
        assert_eq!(depth, 0);
        assert_eq!(in_bits, 8 * BOOT_KEYBOARD_REPORT as u32);
        assert_eq!(out_bits, 8);
    }
}
