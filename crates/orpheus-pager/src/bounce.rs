//! Running a faulting instruction from a trampoline (PG-2, 2026-11-27).
//!
//! Cold code may execute from the rodata window (02 §3.6), where it shares
//! the two SRAM3 slots with the constants. An instruction there can read a
//! constant whose line maps onto the very physical line that holds the
//! instruction: loading the data would overwrite the code being executed
//! (the code window never has this: no constants live in SRAM2). Instead of
//! giving up ([`crate::Fatal::LineConflict`]), the MemManage handler copies
//! the instruction (and, inside an IT block, the rest of the block) into a
//! small trampoline in resident RAM, loads the data line (unmapping the
//! code line), and returns into the trampoline:
//!
//! ```text
//!   [IT <state>]       only inside an IT block: the same IT state again
//!   <instruction>      the faulting load, now with its data mapped
//!   [<rest of block>]  inside an IT block
//!   [nop]              to align the jump
//!   ldr.w pc, [pc]     back to the instruction after them (its line faults
//!   .word resume | 1   back in, taking the data line again)
//! ```
//!
//! Only instructions that run the same at any address are copied: the
//! faulting one must be a load (16- and 32-bit LDR/LDRB/LDRH/LDRSB/LDRSH,
//! LDRD, LDM/POP, VLDR/VLDM) whose base register is not the PC and that
//! does not load the PC; the rest of an IT block may also be ordinary data
//! processing, stores and VFP instructions without the PC. Anything else is
//! refused, which the handler treats as fatal, as before PG-2. The last
//! instruction of the block may also be a return (`BX Rm`, `POP {…, PC}`:
//! [`exit_ok`]). A PC-relative
//! load cannot need this: its literal lies in the same section, hence in the
//! same 16 KiB group, on another physical line.
//!
//! Pure functions over halfwords, tested on the host against encodings from
//! the LLVM assembler; the device side (`atj2157_hal::pager`) owns the
//! buffers, one per exception nesting depth (a trampoline in use is never
//! reused: whatever preempts it runs at a greater depth).

/// Halfwords of one trampoline buffer (32 bytes, 4-byte aligned).
pub const SLOT_HALFWORDS: usize = 16;
/// Halfwords of code the builder may read from the faulting PC on: four
/// 32-bit instructions (an IT block).
pub const FETCH_HALFWORDS: usize = 8;

/// `NOP` (T1).
const NOP: u16 = 0xBF00;
/// `LDR.W PC, [PC, #0]` (T2, literal): at a word-aligned address it loads
/// the word right after it.
const LDR_PC_PC: [u16; 2] = [0xF8DF, 0xF000];
/// xPSR bits holding the IT state (`IT[1:0]` = bits 26:25, `IT[7:2]` =
/// bits 15:10).
const XPSR_IT: u32 = 0x0600_FC00;

/// Why an instruction cannot be bounced.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The faulting instruction is not a load the trampoline can run.
    NotALoad,
    /// An instruction later in the IT block cannot be copied.
    ItBlock,
    /// The xPSR's IT/ICI bits are not an IT state.
    State,
    /// Fewer halfwords than the instructions need.
    Short,
}

/// A trampoline: `code[..len]` to copy into a 4-byte-aligned buffer, and
/// the frame to return with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Trampoline {
    pub code: [u16; SLOT_HALFWORDS],
    pub len: usize,
    /// Where the trampoline continues: the instruction after the copied
    /// ones.
    pub resume: u32,
    /// The stacked xPSR to return with (IT state cleared: the trampoline
    /// sets it again itself).
    pub xpsr: u32,
}

/// Whether `hw` is the first halfword of a 32-bit instruction.
#[must_use]
pub const fn is32(hw: u16) -> bool {
    (hw >> 11) >= 0b11101
}

/// The IT state (`ITSTATE[7:0]`) of a stacked xPSR.
#[must_use]
pub const fn it_state(xpsr: u32) -> u8 {
    (((xpsr >> 25) & 0x3) | ((xpsr >> 8) & 0xFC)) as u8
}

const fn rn(h1: u16) -> u16 {
    h1 & 0xF
}

/// VLDM/VSTM/VLDR/VSTR addressing modes that exist (P, U, W of the
/// extension register load/store encodings).
const fn vfp_mode_ok(h1: u16) -> bool {
    let p = (h1 >> 8) & 1;
    let u = (h1 >> 7) & 1;
    let w = (h1 >> 5) & 1;
    (p == 0 && u == 1) || (p == 1 && w == 0) || (p == 1 && u == 0 && w == 1)
}

/// A load that the trampoline can run: reads memory through a base
/// register other than the PC and does not write the PC. `h2` is the
/// second halfword of a 32-bit instruction (ignored otherwise).
#[must_use]
pub const fn load_ok(h1: u16, h2: u16) -> bool {
    if !is32(h1) {
        return match h1 >> 11 {
            // LDR, LDRB, LDRH (immediate), LDR (SP), LDM.
            0b01101 | 0b01111 | 0b10001 | 0b10011 | 0b11001 => true,
            // Register offset: LDRSB, LDR, LDRH, LDRB, LDRSH.
            0b01010 | 0b01011 => (h1 >> 9) & 7 >= 3,
            // POP without the PC.
            0b10111 => h1 & 0xFE00 == 0xBC00 && h1 & 0x100 == 0,
            _ => false,
        };
    }
    let rt = h2 >> 12;
    if h1 & 0xFE10 == 0xF810 {
        // LDR/LDRB/LDRH/LDRSB/LDRSH (immediate, register, unprivileged).
        let size = (h1 >> 5) & 3;
        let signed = h1 & 0x100 != 0;
        return size != 3 && !(size == 2 && signed) && rn(h1) != 15 && rt != 15;
    }
    if h1 & 0xFE50 == 0xE850 && h1 & 0x120 != 0 {
        // LDRD (immediate; P = 0 and W = 0 is LDREX and TBB/TBH).
        let rt2 = (h2 >> 8) & 0xF;
        return rn(h1) != 15 && rt != 15 && rt != 13 && rt2 != 15 && rt2 != 13;
    }
    if h1 & 0xFFD0 == 0xE890 || h1 & 0xFFD0 == 0xE910 {
        // LDM/LDMIA, LDMDB (POP.W): neither the PC nor the SP in the list.
        return rn(h1) != 15 && h2 & 0xA000 == 0;
    }
    if h1 & 0xFE10 == 0xEC10 && h2 & 0x0E00 == 0x0A00 {
        // VLDR, VLDM (VPOP).
        return rn(h1) != 15 && vfp_mode_ok(h1);
    }
    false
}

/// Whether the data-processing operand fields name the PC only where it
/// means "no register": `Rd` = PC for TST/TEQ/CMN/CMP (flags only), `Rn` =
/// PC for MOV/MVN (ORR/ORN without a first operand).
const fn dp_regs_ok(op: u16, s: bool, rn: u16, rd: u16) -> bool {
    let rd_ok = rd != 15 || (s && matches!(op, 0b0000 | 0b0100 | 0b1000 | 0b1101));
    let rn_ok = rn != 15 || matches!(op, 0b0010 | 0b0011);
    rd_ok && rn_ok
}

/// An instruction later in an IT block that the trampoline can copy: no
/// branch, nothing that reads or writes the PC.
#[must_use]
pub const fn follower_ok(h1: u16, h2: u16) -> bool {
    if load_ok(h1, h2) {
        return true;
    }
    if !is32(h1) {
        if h1 < 0x4400 {
            // Shifts, add/subtract, move/compare immediate, data processing.
            return true;
        }
        if h1 & 0xFC00 == 0x4400 {
            // ADD/CMP/MOV (high registers); BX/BLX.
            let op = (h1 >> 8) & 3;
            let rdn = (h1 & 7) | ((h1 >> 4) & 8);
            let rm = (h1 >> 3) & 0xF;
            return op != 3 && rdn != 15 && rm != 15;
        }
        if h1 >= 0x5000 && h1 < 0xA000 {
            // Loads and stores (register, immediate, SP-relative).
            return true;
        }
        if h1 & 0xF800 == 0xA800 || h1 & 0xF000 == 0xC000 {
            // ADD Rd, SP, #imm; STM/LDM.
            return true;
        }
        return match h1 & 0xFF00 {
            // ADD/SUB SP, #imm; SXTH/SXTB/UXTH/UXTB.
            0xB000 | 0xB200 => true,
            // PUSH.
            0xB400 | 0xB500 => true,
            // REV, REV16, REVSH (`10` is undefined).
            0xBA00 => (h1 >> 6) & 3 != 2,
            // POP without the PC.
            0xBC00 => true,
            _ => false,
        };
    }
    let rd = (h2 >> 8) & 0xF;
    let rt = h2 >> 12;
    let rm = h2 & 0xF;
    if h1 & 0xFF10 == 0xF800 {
        // STR/STRB/STRH.
        return (h1 >> 5) & 3 != 3 && rn(h1) != 15 && rt != 15;
    }
    if h1 & 0xFE50 == 0xE840 && h1 & 0x120 != 0 {
        // STRD (immediate).
        return rn(h1) != 15;
    }
    if h1 & 0xFFD0 == 0xE880 || h1 & 0xFFD0 == 0xE900 {
        // STM/STMIA, STMDB (PUSH.W).
        return rn(h1) != 15 && h2 & 0x8000 == 0;
    }
    if h1 & 0xFE10 == 0xEC00 && h2 & 0x0E00 == 0x0A00 && h1 & 0xFFE0 != 0xEC40 {
        // VSTR, VSTM (VPUSH); not the 64-bit core register transfers.
        return rn(h1) != 15 && vfp_mode_ok(h1);
    }
    if h1 & 0xFFE0 == 0xEC40 && h2 & 0x0E00 == 0x0A00 {
        // VMOV between two core registers and two singles / a double.
        return rt != 15 && rn(h1) != 15;
    }
    if h1 & 0xFA00 == 0xF000 && h2 & 0x8000 == 0 {
        // Data processing, modified immediate.
        return dp_regs_ok((h1 >> 5) & 0xF, h1 & 0x10 != 0, rn(h1), rd);
    }
    if h1 & 0xFA00 == 0xF200 && h2 & 0x8000 == 0 {
        // Data processing, plain binary immediate: MOVW, MOVT and BFC have
        // no first operand (`Rn` = PC there means none); ADR has the PC.
        let op = (h1 >> 4) & 0x1F;
        return rd != 15 && (rn(h1) != 15 || matches!(op, 0b00100 | 0b01100 | 0b10110));
    }
    if h1 & 0xFE00 == 0xEA00 {
        // Data processing, shifted register.
        return rm != 15 && dp_regs_ok((h1 >> 5) & 0xF, h1 & 0x10 != 0, rn(h1), rd);
    }
    if h1 & 0xFF00 == 0xFA00 && h2 & 0xF000 == 0xF000 {
        // Register shifts, extends (`Rn` = PC: no add), parallel and
        // miscellaneous operations.
        let extend_alone = h1 & 0x80 == 0 && h2 & 0x80 != 0;
        return rd != 15 && rm != 15 && (rn(h1) != 15 || extend_alone);
    }
    if h1 & 0xFF80 == 0xFB00 {
        // Multiply and multiply-accumulate (`Ra` = PC: MUL).
        return rd != 15 && rn(h1) != 15 && rm != 15;
    }
    if h1 & 0xFF80 == 0xFB80 {
        // Long multiply; SDIV/UDIV (their `RdLo` field is fixed to ones).
        let op1 = (h1 >> 4) & 7;
        let rdlo_ok = op1 == 0b001 || op1 == 0b011 || rt != 15;
        return rdlo_ok && rd != 15 && rn(h1) != 15 && rm != 15;
    }
    if h1 & 0xFF00 == 0xEE00 && h2 & 0x0E00 == 0x0A00 {
        if h2 & 0x10 == 0 {
            // VFP data processing.
            return true;
        }
        // VMOV core <-> single, VMRS/VMSR: `Rt` = PC only for
        // `VMRS APSR_nzcv, FPSCR` (flags).
        return rt != 15 || (h1 == 0xEEF1 && h2 & 0x0FFF == 0x0A10);
    }
    false
}

/// A write cursor over a trampoline buffer: no index, so no bounds-check
/// panic in the fault path (`check-resident`); beyond the buffer a halfword
/// is dropped (the builder never writes more than 14).
struct Cursor<'a> {
    buf: &'a mut [u16; SLOT_HALFWORDS],
    len: usize,
}

impl Cursor<'_> {
    fn push(&mut self, hw: u16) {
        if let Some(x) = self.buf.get_mut(self.len) {
            *x = hw;
        }
        self.len += 1;
    }
}

/// A branch that may end an IT block in the trampoline: it leaves for a
/// target held in a register or on the stack (a conditional return), never
/// to come back into the trampoline. `BX Rm` (not the PC), `POP {…, PC}`,
/// `POP.W {…, PC}`. Not `BLX`: the call would return into a slot that a
/// bounce in the callee (same depth) may have rewritten.
#[must_use]
pub const fn exit_ok(h1: u16, h2: u16) -> bool {
    if !is32(h1) {
        return (h1 & 0xFF87 == 0x4700 && (h1 >> 3) & 0xF != 15) || h1 & 0xFF00 == 0xBD00;
    }
    h1 == 0xE8BD && h2 & 0x8000 != 0 && h2 & 0x2000 == 0
}

/// Build the trampoline for the instruction at `pc` that faulted with the
/// stacked `xpsr`; `code` holds the halfwords at `pc` onwards (at least
/// [`FETCH_HALFWORDS`] inside an IT block, else the instruction's own).
pub fn build(pc: u32, xpsr: u32, code: &[u16]) -> Result<Trampoline, Refusal> {
    let mut t = Trampoline { code: [NOP; SLOT_HALFWORDS], len: 0, resume: pc, xpsr };
    let (len, resume, xpsr) = build_into(&mut t.code, pc, xpsr, code)?;
    t.len = len;
    t.resume = resume;
    t.xpsr = xpsr;
    Ok(t)
}

/// As [`build`], straight into the buffer the trampoline runs from (the
/// fault handler's: no copy on its stack). Returns the halfwords written,
/// where the trampoline continues and the xPSR to return with. On a
/// refusal the buffer holds a partial trampoline: never run it.
pub fn build_into(
    buf: &mut [u16; SLOT_HALFWORDS],
    pc: u32,
    xpsr: u32,
    code: &[u16],
) -> Result<(usize, u32, u32), Refusal> {
    let it = it_state(xpsr);
    let mask = it & 0xF;
    if mask == 0 && it != 0 {
        return Err(Refusal::State);
    }
    if mask != 0 && it >> 4 == 0xF {
        return Err(Refusal::State);
    }
    // Instructions to copy: this one, and inside an IT block the rest of it.
    let count = if mask == 0 { 1 } else { 4 - mask.trailing_zeros() as usize };
    let mut t = Cursor { buf, len: 0 };
    if mask != 0 {
        t.push(0xBF00 | u16::from(it));
    }
    let mut at = 0;
    for i in 0..count {
        let h1 = *code.get(at).ok_or(Refusal::Short)?;
        let wide = is32(h1);
        let h2 = if wide { *code.get(at + 1).ok_or(Refusal::Short)? } else { 0 };
        let ok =
            if i == 0 { load_ok(h1, h2) } else { follower_ok(h1, h2) || (i + 1 == count && exit_ok(h1, h2)) };
        if !ok {
            return Err(if i == 0 { Refusal::NotALoad } else { Refusal::ItBlock });
        }
        t.push(h1);
        at += 1;
        if wide {
            t.push(h2);
            at += 1;
        }
    }
    if t.len % 2 == 1 {
        t.push(NOP);
    }
    let resume = pc.wrapping_add(2 * at as u32);
    let back = resume | 1;
    t.push(LDR_PC_PC[0]);
    t.push(LDR_PC_PC[1]);
    t.push(back as u16);
    t.push((back >> 16) as u16);
    // Unused halfwords: NOPs (a trampoline never runs into them).
    let len = t.len;
    while t.len < SLOT_HALFWORDS {
        t.push(NOP);
    }
    Ok((len, resume, xpsr & !XPSR_IT))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encodings from `llvm-mc -triple=thumbv7em -mattr=+vfp4d16sp
    /// -show-encoding` (halfwords in instruction order).
    const LOADS: &[(&str, &[u16])] = &[
        ("ldr r0, [r1, #4]", &[0x6848]),
        ("ldrb r2, [r3, #1]", &[0x785A]),
        ("ldrh r4, [r5, #2]", &[0x886C]),
        ("ldr r0, [sp, #8]", &[0x9802]),
        ("ldr r0, [r1, r2]", &[0x5888]),
        ("ldrb r0, [r1, r2]", &[0x5C88]),
        ("ldrh r0, [r1, r2]", &[0x5A88]),
        ("ldrsb r0, [r1, r2]", &[0x5688]),
        ("ldrsh r0, [r1, r2]", &[0x5E88]),
        ("ldm r1!, {r2, r3}", &[0xC90C]),
        ("pop {r4, r5}", &[0xBC30]),
        ("ldr.w r8, [r9, #0x123]", &[0xF8D9, 0x8123]),
        ("ldr r0, [r1, #-4]", &[0xF851, 0x0C04]),
        ("ldr r0, [r1], #4", &[0xF851, 0x0B04]),
        ("ldr r0, [r1, #4]!", &[0xF851, 0x0F04]),
        ("ldr.w r0, [r1, r2, lsl #2]", &[0xF851, 0x0022]),
        ("ldrb.w r0, [r11, #0x7ff]", &[0xF89B, 0x07FF]),
        ("ldrh.w r0, [r12, #0x10]", &[0xF8BC, 0x0010]),
        ("ldrsb.w r0, [r1, #3]", &[0xF991, 0x0003]),
        ("ldrsh.w r0, [r1, #6]", &[0xF9B1, 0x0006]),
        ("ldrd r0, r1, [r2, #8]", &[0xE9D2, 0x0102]),
        ("ldrd r0, r1, [r2], #8", &[0xE8F2, 0x0102]),
        ("ldm.w r0, {r1, r2, r8}", &[0xE890, 0x0106]),
        ("ldmdb r0!, {r1, r2}", &[0xE930, 0x0006]),
        ("pop.w {r4, r5, r6, r7, r8, lr}", &[0xE8BD, 0x41F0]),
        ("vldr s0, [r0, #4]", &[0xED90, 0x0A01]),
        ("vldr d1, [r2, #-8]", &[0xED12, 0x1B02]),
        ("vldmia r0, {s0, s1, s2}", &[0xEC90, 0x0A03]),
        ("vldmia r0!, {d0, d1}", &[0xECB0, 0x0B04]),
        ("vpop {d8}", &[0xECBD, 0x8B02]),
        ("vldmdb r1!, {s4, s5}", &[0xED31, 0x2A02]),
    ];

    const NOT_LOADS: &[(&str, &[u16])] = &[
        ("ldr r0, [pc, #8]", &[0x4802]),
        ("ldr.w r0, [pc, #-8]", &[0xF85F, 0x0008]),
        ("ldr.w pc, [r0, #4]", &[0xF8D0, 0xF004]),
        ("pop {r4, pc}", &[0xBD10]),
        ("pop.w {r4, r5, pc}", &[0xE8BD, 0x8030]),
        ("ldm.w r0, {r1, pc}", &[0xE890, 0x8002]),
        ("ldrd r0, r1, [pc, #8]", &[0xE9DF, 0x0102]),
        ("ldrex r0, [r1]", &[0xE851, 0x0F00]),
        ("tbb [r0, r1]", &[0xE8D0, 0xF001]),
        ("tbh [r0, r1, lsl #1]", &[0xE8D0, 0xF011]),
        ("pld [r0]", &[0xF890, 0xF000]),
        ("vldr s0, [pc, #4]", &[0xED9F, 0x0A01]),
        ("str r0, [r1]", &[0x6008]),
        ("str.w r0, [r1, #0x100]", &[0xF8C1, 0x0100]),
        ("adds r0, r1, r2", &[0x1888]),
        ("bx lr", &[0x4770]),
        ("vstr s0, [r0]", &[0xED80, 0x0A00]),
        ("vmov r0, r1, d0", &[0xEC51, 0x0B10]),
    ];

    /// Ordinary instructions inside an IT block.
    const FOLLOWERS: &[(&str, &[u16])] = &[
        ("adds r0, #1", &[0x3001]),
        ("movs r1, r2", &[0x0011]),
        ("ands r0, r1", &[0x4008]),
        ("add r8, r1", &[0x4488]),
        ("mov r9, r10", &[0x46D1]),
        ("cmp r8, r9", &[0x45C8]),
        ("str r0, [r1, #4]", &[0x6048]),
        ("strb r0, [r1, r2]", &[0x5488]),
        ("str r0, [sp, #4]", &[0x9001]),
        ("add r0, sp, #8", &[0xA802]),
        ("sub sp, #8", &[0xB082]),
        ("uxtb r0, r1", &[0xB2C8]),
        ("push {r4, lr}", &[0xB510]),
        ("rev r0, r1", &[0xBA08]),
        ("stm r0!, {r1, r2}", &[0xC006]),
        ("str.w r0, [r1, #0x100]", &[0xF8C1, 0x0100]),
        ("strd r0, r1, [r2, #8]", &[0xE9C2, 0x0102]),
        ("stmdb sp!, {r4, r5, lr}", &[0xE92D, 0x4030]),
        ("vstr s0, [r0]", &[0xED80, 0x0A00]),
        ("vpush {d8}", &[0xED2D, 0x8B02]),
        ("vmov r0, r1, d0", &[0xEC51, 0x0B10]),
        ("add.w r0, r1, #0x100", &[0xF501, 0x7080]),
        ("cmp.w r0, #0x100", &[0xF5B0, 0x7F80]),
        ("tst.w r0, #1", &[0xF010, 0x0F01]),
        ("mov.w r0, #0x100", &[0xF44F, 0x7080]),
        ("mvn r0, #1", &[0xF06F, 0x0001]),
        ("movw r0, #0x1234", &[0xF241, 0x2034]),
        ("movt r0, #0x1234", &[0xF2C1, 0x2034]),
        ("ubfx r0, r1, #2, #3", &[0xF3C1, 0x0082]),
        ("bfc r0, #4, #8", &[0xF36F, 0x100B]),
        ("add.w r0, r1, r2, lsl #2", &[0xEB01, 0x0082]),
        ("cmp.w r0, r1, lsl #2", &[0xEBB0, 0x0F81]),
        ("mov.w r0, r1, lsl #3", &[0xEA4F, 0x00C1]),
        ("lsl.w r0, r1, r2", &[0xFA01, 0xF002]),
        ("sxtb.w r0, r1", &[0xFA4F, 0xF081]),
        ("uxtah r0, r1, r2", &[0xFA11, 0xF082]),
        ("clz r0, r1", &[0xFAB1, 0xF081]),
        ("mul r0, r1, r2", &[0xFB01, 0xF002]),
        ("mla r0, r1, r2, r3", &[0xFB01, 0x3002]),
        ("umull r0, r1, r2, r3", &[0xFBA2, 0x0103]),
        ("sdiv r0, r1, r2", &[0xFB91, 0xF0F2]),
        ("vadd.f32 s0, s1, s2", &[0xEE30, 0x0A81]),
        ("vmov r0, s1", &[0xEE10, 0x0A90]),
        ("vmrs APSR_nzcv, fpscr", &[0xEEF1, 0xFA10]),
    ];

    const NOT_FOLLOWERS: &[(&str, &[u16])] = &[
        ("ldr r0, [pc, #8]", &[0x4802]),
        ("adr r0, #8", &[0xA002]),
        ("add r0, pc", &[0x4478]),
        ("mov pc, r0", &[0x4687]),
        ("bx r0", &[0x4700]),
        ("blx r0", &[0x4780]),
        ("pop {pc}", &[0xBD00]),
        ("b #4", &[0xE002]),
        ("bl #4", &[0xF000, 0xF802]),
        ("b.w #4", &[0xF000, 0xB802]),
        ("bkpt #0", &[0xBE00]),
        ("svc #0", &[0xDF00]),
        ("nop", &[0xBF00]),
        ("str.w r0, [pc, #4]", &[0xF8CF, 0x0004]),
        ("add.w pc, r0, #4 (hand-encoded: the assembler refuses it)", &[0xF100, 0x0F04]),
        ("addw r0, pc, #4", &[0xF20F, 0x0004]),
        ("mov.w pc, r0 (hand-encoded: the assembler refuses it)", &[0xEA4F, 0x0F00]),
        ("add.w r0, r1, pc (hand-encoded: the assembler refuses it)", &[0xEB01, 0x000F]),
        ("mrs r0, apsr", &[0xF3EF, 0x8000]),
        ("vmov pc, s0", &[0xEE10, 0xFA10]),
    ];

    #[test]
    fn loads_are_bounced_and_others_are_not() {
        for (name, h) in LOADS {
            let h2 = h.get(1).copied().unwrap_or(0);
            assert_eq!(is32(h[0]), h.len() == 2, "{name}: width");
            assert!(load_ok(h[0], h2), "{name}");
            assert!(follower_ok(h[0], h2), "{name}: in an IT block");
        }
        for (name, h) in NOT_LOADS {
            let h2 = h.get(1).copied().unwrap_or(0);
            assert_eq!(is32(h[0]), h.len() == 2, "{name}: width");
            assert!(!load_ok(h[0], h2), "{name}");
        }
    }

    #[test]
    fn it_block_followers() {
        for (name, h) in FOLLOWERS {
            let h2 = h.get(1).copied().unwrap_or(0);
            assert_eq!(is32(h[0]), h.len() == 2, "{name}: width");
            assert!(follower_ok(h[0], h2), "{name}");
        }
        for (name, h) in NOT_FOLLOWERS {
            let h2 = h.get(1).copied().unwrap_or(0);
            assert_eq!(is32(h[0]), h.len() == 2, "{name}: width");
            assert!(!follower_ok(h[0], h2), "{name}");
        }
    }

    const T_BIT: u32 = 1 << 24;

    /// The stacked xPSR for an IT state.
    fn xpsr_it(it: u8) -> u32 {
        T_BIT | (u32::from(it & 3) << 25) | (u32::from(it >> 2) << 10)
    }

    #[test]
    fn a_plain_load_jumps_back_after_itself() {
        let pc = 0x0034_2A10;
        // ldr r0, [r1, #4]; (next instruction not copied)
        let t = build(pc, T_BIT | 0x21, &[0x6848, 0xFFFF]).unwrap();
        let back = (pc + 2) | 1;
        assert_eq!(
            &t.code[..t.len],
            &[0x6848, NOP, 0xF8DF, 0xF000, back as u16, (back >> 16) as u16],
            "load, align, ldr.w pc, [pc]; the word"
        );
        assert_eq!(t.resume, pc + 2);
        assert_eq!(t.xpsr, T_BIT | 0x21, "exception number bits kept");
        // A 32-bit load needs no padding.
        let t = build(pc, T_BIT, &[0xF8D9, 0x8123]).unwrap();
        assert_eq!(&t.code[..4], &[0xF8D9, 0x8123, 0xF8DF, 0xF000]);
        assert_eq!((t.len, t.resume), (6, pc + 4));
    }

    #[test]
    fn an_it_block_is_copied_whole_with_its_state() {
        // `itt ne; ldrne r0, [r1]; addne r0, #1`: the fault is on the
        // load, the first instruction of the block (ITSTATE = NE, mask
        // `x100` with T: 0x14).
        let it = 0x14;
        let pc = 0x0030_0102;
        let code = [0x6808, 0x3001, 0x4770, 0, 0, 0, 0, 0];
        let t = build(pc, xpsr_it(it), &code).unwrap();
        assert_eq!(&t.code[..4], &[0xBF14, 0x6808, 0x3001, NOP]);
        assert_eq!(&t.code[4..6], &LDR_PC_PC);
        assert_eq!(t.resume, pc + 4, "after the block, not the bx lr");
        assert_eq!(t.xpsr & XPSR_IT, 0);
        assert_eq!(t.xpsr & T_BIT, T_BIT);
        // The last instruction of a block: ITSTATE mask 0b1000.
        let t = build(pc, xpsr_it(0x08), &[0x6808, 0x3001]).unwrap();
        assert_eq!(&t.code[..3], &[0xBF08, 0x6808, 0xF8DF], "IT EQ, the load, ldr.w (aligned)");
        assert_eq!(t.resume, pc + 2);
        // Four instructions, two of them 32-bit: IT + 1 + 2 + 2 + 1 = 7
        // halfwords, padded to 8, then the jump and its word: 12.
        let code = [0x6808, 0xF8C1, 0x0100, 0xF501, 0x7080, 0x3001, 0, 0];
        let t = build(pc, xpsr_it(0x01), &code).unwrap();
        assert_eq!(t.len, 12);
        assert_eq!(t.resume, pc + 12);
        assert!(t.len <= SLOT_HALFWORDS);
    }

    /// `itttt cc; ldrbcc; ldrbcc; orrcc.w; popcc {r7, pc}` (the library's
    /// `format::rd16`, 2026-11-27: the audit's only refusals): a block may
    /// end in a return.
    #[test]
    fn an_it_block_may_end_in_a_return() {
        let pc = 0x003E_B0CA;
        let code = [0x5C81, 0x5CC0, 0xEA41, 0x2000, 0xBD80, 0, 0, 0];
        // The fault on the first load: ITSTATE CC with three more (0x3F
        // after `itttt cc`, 0xBF3F).
        let t = build(pc, xpsr_it(0x3F), &code).unwrap();
        assert_eq!(&t.code[..6], &[0xBF3F, 0x5C81, 0x5CC0, 0xEA41, 0x2000, 0xBD80]);
        assert_eq!(t.resume, pc + 10);
        // On the second: the state has advanced (0x3E).
        let t = build(pc + 2, xpsr_it(0x3E), &code[1..]).unwrap();
        assert_eq!(&t.code[..5], &[0xBF3E, 0x5CC0, 0xEA41, 0x2000, 0xBD80]);
        assert!(exit_ok(0x4770, 0), "bx lr");
        assert!(exit_ok(0xE8BD, 0x8030), "pop.w {{r4, r5, pc}}");
        assert!(!exit_ok(0x4780, 0), "blx r0");
        assert!(!exit_ok(0x4778, 0), "bx pc");
        // Not before the end of the block.
        assert_eq!(build(pc, xpsr_it(0x3F), &[0x5C81, 0xBD80, 0x5CC0, 0x3001]), Err(Refusal::ItBlock));
    }

    #[test]
    fn refusals() {
        let pc = 0x0030_0000;
        assert_eq!(build(pc, T_BIT, &[0x4802]), Err(Refusal::NotALoad), "ldr literal");
        assert_eq!(build(pc, T_BIT, &[0xF8D0]), Err(Refusal::Short));
        assert_eq!(build(pc, T_BIT, &[]), Err(Refusal::Short));
        // A branch later in the block.
        assert_eq!(build(pc, xpsr_it(0x14), &[0x6808, 0x4780]), Err(Refusal::ItBlock), "blx r0");
        // ICI bits (an interrupted LDM's continuation), not an IT state.
        assert_eq!(build(pc, T_BIT | (0b0100 << 12), &[0x6808]), Err(Refusal::State));
    }

    #[test]
    fn it_state_round_trips_through_the_xpsr() {
        for it in [0x08u8, 0x14, 0x1C, 0x2A, 0xE8, 0x31, 0x0F] {
            assert_eq!(it_state(xpsr_it(it)), it);
        }
        assert_eq!(it_state(T_BIT | 0x2F), 0, "exception number is not IT state");
    }
}
