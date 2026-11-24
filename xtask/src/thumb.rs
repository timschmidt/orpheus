//! The few Thumb-2 encodings the link analysis needs (ARMv7-M ARM §A7):
//! branch and literal targets at relocation sites (read from the *linked*
//! bytes, so the result is the final address), and indirect branches.

/// ELF relocation types (AAELF32) that appear in Cortex-M Rust code.
pub mod rel {
    pub const NONE: u32 = 0;
    pub const ABS32: u32 = 2;
    pub const REL32: u32 = 3;
    pub const THM_CALL: u32 = 10;
    pub const THM_PC8: u32 = 11;
    pub const THM_JUMP24: u32 = 30;
    pub const THM_ALU_PREL_11_0: u32 = 35;
    pub const TARGET1: u32 = 38;
    pub const V4BX: u32 = 40;
    pub const PREL31: u32 = 42;
    pub const THM_MOVW_ABS_NC: u32 = 47;
    pub const THM_MOVT_ABS: u32 = 48;
    pub const THM_JUMP19: u32 = 51;
    pub const THM_PC12: u32 = 54;
    pub const THM_JUMP11: u32 = 102;
    pub const THM_JUMP8: u32 = 103;
}

fn sext(v: u32, bits: u32) -> u32 {
    let shift = 32 - bits;
    (((v << shift) as i32) >> shift) as u32
}

/// Length of the Thumb instruction starting with halfword `hw1`.
#[must_use]
pub fn insn_len(hw1: u16) -> u32 {
    if matches!(hw1 >> 11, 0b11101..=0b11111) { 4 } else { 2 }
}

/// Target of `BL`/`B.W` (T4) at `place`. `BLX` (to ARM state) is aligned.
#[must_use]
pub fn bl_target(place: u32, hw1: u16, hw2: u16) -> u32 {
    let (h1, h2) = (u32::from(hw1), u32::from(hw2));
    let s = (h1 >> 10) & 1;
    let i1 = !((h2 >> 13) ^ s) & 1;
    let i2 = !((h2 >> 11) ^ s) & 1;
    let imm = (s << 24) | (i1 << 23) | (i2 << 22) | ((h1 & 0x3FF) << 12) | ((h2 & 0x7FF) << 1);
    let off = sext(imm, 25);
    let base = if h2 & 0x1000 == 0 { (place + 4) & !3 } else { place + 4 };
    base.wrapping_add(off)
}

/// Target of `B<c>.W` (T3).
#[must_use]
pub fn bcond_w_target(place: u32, hw1: u16, hw2: u16) -> u32 {
    let (h1, h2) = (u32::from(hw1), u32::from(hw2));
    let imm = (((h1 >> 10) & 1) << 20)
        | (((h2 >> 11) & 1) << 19)
        | (((h2 >> 13) & 1) << 18)
        | ((h1 & 0x3F) << 12)
        | ((h2 & 0x7FF) << 1);
    (place + 4).wrapping_add(sext(imm, 21))
}

/// Target of `B` (T2, 16 bit).
#[must_use]
pub fn b16_target(place: u32, hw: u16) -> u32 {
    (place + 4).wrapping_add(sext((u32::from(hw) & 0x7FF) << 1, 12))
}

/// Target of `B<c>` (T1, 16 bit).
#[must_use]
pub fn bcond16_target(place: u32, hw: u16) -> u32 {
    (place + 4).wrapping_add(sext((u32::from(hw) & 0xFF) << 1, 9))
}

/// Literal address of `LDR rt, [pc, #imm8*4]` (T1).
#[must_use]
pub fn ldr16_lit_target(place: u32, hw: u16) -> u32 {
    ((place + 4) & !3) + ((u32::from(hw) & 0xFF) << 2)
}

/// Literal address of `LDR.W rt, [pc, #±imm12]` (T2) and friends.
#[must_use]
pub fn ldr_w_lit_target(place: u32, hw1: u16, hw2: u16) -> u32 {
    let base = (place + 4) & !3;
    let imm = u32::from(hw2) & 0xFFF;
    if hw1 & 0x80 != 0 { base + imm } else { base.wrapping_sub(imm) }
}

/// Address computed by `ADR.W` (`ADDW`/`SUBW rd, pc, #imm12`).
#[must_use]
pub fn adr_w_target(place: u32, hw1: u16, hw2: u16) -> u32 {
    let base = (place + 4) & !3;
    let imm =
        ((u32::from(hw1) >> 10 & 1) << 11) | ((u32::from(hw2) >> 12 & 7) << 8) | (u32::from(hw2) & 0xFF);
    if hw1 & 0x00A0 == 0x00A0 { base.wrapping_sub(imm) } else { base + imm }
}

/// The 16-bit immediate of `MOVW`/`MOVT` (T3/T1) and its destination
/// register.
#[must_use]
pub fn mov_imm16(hw1: u16, hw2: u16) -> (u16, u8) {
    let (h1, h2) = (hw1, hw2);
    let imm = ((h1 & 0xF) << 12) | (((h1 >> 10) & 1) << 11) | (((h2 >> 12) & 7) << 8) | (h2 & 0xFF);
    (imm, ((h2 >> 8) & 0xF) as u8)
}

/// An indirect branch: `BLX rm`, `BX rm` (other than `BX LR`), `MOV PC, rm`,
/// `LDR.W PC, [..]`. Returns a short description.
#[must_use]
pub fn indirect(hw1: u16, hw2: u16) -> Option<String> {
    if insn_len(hw1) == 2 {
        let rm = (hw1 >> 3) & 0xF;
        return match hw1 & 0xFF87 {
            0x4780 => Some(format!("blx r{rm}")),
            0x4700 if rm != 14 => Some(format!("bx r{rm}")),
            0x4687 => Some(format!("mov pc, r{rm}")),
            _ => None,
        };
    }
    // LDR (immediate T3/T4, literal T2) with Rt = PC; not POP (LDR PC,
    // [SP], #4 is a return).
    if hw1 & 0xFF70 == 0xF850 && hw2 >> 12 == 0xF {
        let rn = hw1 & 0xF;
        let is_pop = rn == 13 && hw1 & 0xFFF0 == 0xF850 && hw2 & 0x0FFF == 0x0B04;
        if !is_pop {
            return Some(if rn == 15 { "ldr pc, [pc, ..]".into() } else { format!("ldr pc, [r{rn}, ..]") });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encode `BL` (T4) from `place` to `target` (test helper).
    fn enc_bl(place: u32, target: u32) -> (u16, u16) {
        let off = target.wrapping_sub(place + 4);
        let s = (off >> 24) & 1;
        let i1 = (off >> 23) & 1;
        let i2 = (off >> 22) & 1;
        let j1 = (!i1 ^ s) & 1;
        let j2 = (!i2 ^ s) & 1;
        let hw1 = 0xF000 | (s << 10) | ((off >> 12) & 0x3FF);
        let hw2 = 0xD000 | (j1 << 13) | (j2 << 11) | ((off >> 1) & 0x7FF);
        (hw1 as u16, hw2 as u16)
    }

    #[test]
    fn bl_round_trip() {
        for (p, t) in
            [(0x10_0018, 0x20_0000), (0x20_0000, 0x10_0018), (0x10_0000, 0x10_0004), (0x3F_FFF0, 0x10_0000)]
        {
            let (a, b) = enc_bl(p, t);
            assert_eq!(bl_target(p, a, b), t, "{p:#x} -> {t:#x}");
        }
        // `bl .+4` is F000 F800.
        assert_eq!(bl_target(0x1000, 0xF000, 0xF800), 0x1004);
    }

    #[test]
    fn short_branches_and_literals() {
        assert_eq!(b16_target(0x100, 0xE7FE), 0x100); // b .
        assert_eq!(bcond16_target(0x100, 0xD0FE), 0x100); // beq .
        assert_eq!(ldr16_lit_target(0x102, 0x4801), 0x108); // ldr r0, [pc, #4]
        assert_eq!(ldr_w_lit_target(0x100, 0xF8DF, 0x0010), 0x114);
        assert_eq!(ldr_w_lit_target(0x100, 0xF85F, 0x0010), 0xF4);
        // beq.w . (T3, offset -4)
        assert_eq!(bcond_w_target(0x200, 0xF43F, 0xAFFE), 0x200);
    }

    #[test]
    fn movw_movt_immediates() {
        // movw r0, #0x1234 = F241 2034; movt r3, #0x0020 = F2C0 0320.
        assert_eq!(mov_imm16(0xF241, 0x2034), (0x1234, 0));
        assert_eq!(mov_imm16(0xF2C0, 0x0320), (0x0020, 3));
        // movw r1, #0xFFFF = F64F 71FF
        assert_eq!(mov_imm16(0xF64F, 0x71FF), (0xFFFF, 1));
    }

    #[test]
    fn indirect_branches() {
        assert_eq!(indirect(0x4798, 0).as_deref(), Some("blx r3"));
        assert_eq!(indirect(0x4770, 0), None); // bx lr
        assert_eq!(indirect(0x4718, 0).as_deref(), Some("bx r3"));
        assert_eq!(indirect(0x4687, 0).as_deref(), Some("mov pc, r0"));
        assert_eq!(indirect(0xF85D, 0xFB04), None); // pop {pc} as ldr
        assert_eq!(indirect(0xF8D0, 0xF004).as_deref(), Some("ldr pc, [r0, ..]"));
        assert_eq!(insn_len(0xF000), 4);
        assert_eq!(insn_len(0xE7FE), 2);
    }
}
