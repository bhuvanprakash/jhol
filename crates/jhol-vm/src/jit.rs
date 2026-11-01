//! JholVM's native-code compilers for little-endian ARM64 macOS/Linux.
//!
//! Counted numeric loops retain unboxed numbers in registers. Boxed regions
//! extend coverage to internal branches, guarded own-property loads and pure
//! native calls. Whole-function compilation accelerates eligible numeric and
//! captured-cell callees. Unsupported instructions remain interpreted.
//!
//! Entry guards preserve JavaScript numeric semantics. Numeric loops either
//! finish or decline entry; boxed regions commit each bytecode result before
//! the next guard and return the exact instruction index on a side exit.
//! Eligible native calls cannot allocate, collect or call arbitrary JavaScript.
//! Boxed region values remain in the VM stack, visible to GC between entries.
//!
//! Code is written into anonymous RW memory, synchronized with the instruction
//! cache and changed to RX before publication. Mappings belong to their loop or
//! function prototype. Allocation/protection failure is an ordinary JIT miss;
//! unsupported CPUs and operating systems use the same interpreter.

use crate::bytecode::{Ic, Instr, Op};
use crate::objects::PropKey;
use crate::value::Value;

/// Smi-boxed constant: `BOX_TAG | (SUB_SMI << 44)`.
const SMI_BOX: u64 = 0x7FF8_1000_0000_0000;
/// Half-open Smi magnitude limit (±2^43).
const SMI_LIMIT: i64 = 1 << 43;
const SMI_TAG20: u64 = 0x7FF81;
/// `Value::NAN` bits.
const NAN_BITS: u64 = 0x7FF0_0000_0000_0001;
/// Boxed `true` bits.
const TRUE_BITS: u64 = Value::TRUE.0;
const FALSE_BITS: u64 = Value::FALSE.0;
/// Boxed tag top 16 bits.
const BOX_TOP: u64 = 0x7FF8;

// All emitters below produce little-endian AArch64 instructions and use the
// Unix AAPCS64 calling convention. Other hosts retain interpreted execution.
pub(crate) const NATIVE_SUPPORTED: bool = cfg!(all(
    target_arch = "aarch64",
    target_endian = "little",
    any(target_os = "macos", target_os = "linux")
));

pub struct JitLoop {
    pub code: *const u8,
    pub len: usize,
    /// Instruction index the VM continues at when the loop completes.
    pub exit_ip: usize,
}

impl Drop for JitLoop {
    fn drop(&mut self) {
        unsafe { release_exec(self.code as *mut u8, self.len) };
    }
}

#[cfg(all(target_arch = "aarch64", target_os = "macos"))]
extern "C" {
    fn sys_icache_invalidate(start: *mut std::ffi::c_void, len: usize);
}

fn alloc_exec(len: usize) -> *mut u8 {
    if !NATIVE_SUPPORTED || len == 0 {
        return std::ptr::null_mut();
    }
    #[cfg(unix)]
    unsafe {
        // mmap rounds to the host page size and anonymous pages start zeroed.
        let p = libc::mmap(
            std::ptr::null_mut(),
            len,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANON,
            -1,
            0,
        );
        if p == libc::MAP_FAILED {
            return std::ptr::null_mut();
        }
        p.cast()
    }
    #[cfg(not(unix))]
    std::ptr::null_mut()
}

fn seal_exec(p: *mut u8, len: usize) -> bool {
    if !NATIVE_SUPPORTED || p.is_null() || len == 0 {
        return false;
    }
    #[cfg(unix)]
    unsafe {
        synchronize_code(p, len);
        libc::mprotect(p.cast(), len, libc::PROT_READ | libc::PROT_EXEC) == 0
    }
    #[cfg(not(unix))]
    false
}

pub(crate) unsafe fn release_exec(p: *mut u8, len: usize) {
    #[cfg(unix)]
    {
        libc::munmap(p.cast(), len);
    }
    #[cfg(not(unix))]
    {
        let _ = (p, len);
    }
}

#[cfg(unix)]
unsafe fn synchronize_code(p: *mut u8, len: usize) {
    #[cfg(all(target_arch = "aarch64", target_os = "macos"))]
    sys_icache_invalidate(p.cast(), len);
    #[cfg(all(target_arch = "aarch64", target_os = "linux"))]
    {
        use std::arch::asm;
        let cache_type: u64;
        asm!("mrs {cache_type}, ctr_el0", cache_type = out(reg) cache_type,
            options(nostack, preserves_flags));
        let end = p as usize + len;
        let data_line = 4usize << ((cache_type >> 16) & 15);
        let instruction_line = 4usize << (cache_type & 15);
        // Complete data writes before invalidating instruction lines. These
        // asm blocks deliberately retain their compiler memory barriers.
        for line in ((p as usize & !(data_line - 1))..end).step_by(data_line) {
            asm!("dc cvau, {line}", line = in(reg) line, options(nostack, preserves_flags));
        }
        asm!("dsb ish", options(nostack, preserves_flags));
        for line in ((p as usize & !(instruction_line - 1))..end).step_by(instruction_line) {
            asm!("ic ivau, {line}", line = in(reg) line, options(nostack, preserves_flags));
        }
        asm!("dsb ish", "isb", options(nostack, preserves_flags));
    }
    #[cfg(not(all(target_arch = "aarch64", any(target_os = "macos", target_os = "linux"))))]
    {
        let _ = (p, len);
    }
}

#[cfg(test)]
mod executable_memory_tests {
    use super::*;

    #[test]
    fn empty_or_unsupported_code_is_not_published() {
        assert!(alloc_exec(0).is_null());
        assert!(!seal_exec(std::ptr::null_mut(), 0));
        if !NATIVE_SUPPORTED {
            assert!(alloc_exec(8).is_null());
        }
    }

    #[cfg(all(
        target_arch = "aarch64",
        target_endian = "little",
        any(target_os = "macos", target_os = "linux")
    ))]
    #[test]
    fn rewritten_code_is_visible_after_publication() {
        // Exercise repeated RW -> RX transitions on one address, including
        // instructions spanning several cache lines and a non-page-sized map.
        let len = 1028;
        let p = alloc_exec(len);
        assert!(!p.is_null());
        let mapping = JitLoop {
            code: p,
            len,
            exit_ip: 0,
        };
        let entry: unsafe extern "C" fn() -> u32 = unsafe { std::mem::transmute(p) };
        for result in 0..128u32 {
            unsafe {
                assert_eq!(
                    libc::mprotect(p.cast(), len, libc::PROT_READ | libc::PROT_WRITE),
                    0
                );
                for index in 0..len / 4 {
                    p.cast::<u32>().add(index).write(0xd503201fu32.to_le());
                }
                // movz w0, #result; all remaining instructions are nops; ret.
                p.cast::<u32>().write((0x52800000u32 | result << 5).to_le());
                p.add(len - 4).cast::<u32>().write(0xd65f03c0u32.to_le());
                assert!(seal_exec(p, len));
                assert_eq!(entry(), result);
            }
        }
        drop(mapping);
    }
}

struct Asm {
    buf: Vec<u32>,
    fixups: Vec<(usize, usize, u8)>,
    labels: Vec<usize>,
}

impl Asm {
    fn new() -> Self {
        Asm {
            buf: Vec::new(),
            fixups: Vec::new(),
            labels: Vec::new(),
        }
    }
    fn label(&mut self) -> usize {
        self.labels.push(self.buf.len());
        self.labels.len() - 1
    }
    fn bind(&mut self, l: usize) {
        self.labels[l] = self.buf.len();
    }
    fn word(&mut self, w: u32) {
        self.buf.push(w.to_le());
    }
    fn finish(mut self) -> Vec<u32> {
        for (at, l, kind) in self.fixups {
            let off = self.labels[l] as i64 - at as i64;
            let w = &mut self.buf[at];
            let decoded = u32::from_le(*w);
            *w = match kind {
                0 => (decoded & !(0x7FFFF << 5)) | (((off as u32) & 0x7FFFF) << 5),
                1 => (decoded & 0xFC00_0000) | ((off as u32) & 0x3FF_FFFF),
                _ => (decoded & !(0x7FFF << 5)) | (((off as u32) & 0x7FFF) << 5),
            }
            .to_le();
        }
        self.buf
    }
    fn movz(&mut self, rd: u32, imm: u64) {
        self.word(0xD280_0000 | (((imm & 0xFFFF) as u32) << 5) | rd);
    }
    fn mov_const(&mut self, rd: u32, v: u64) {
        let c = [
            (v & 0xFFFF) as u32,
            ((v >> 16) & 0xFFFF) as u32,
            ((v >> 32) & 0xFFFF) as u32,
            ((v >> 48) & 0xFFFF) as u32,
        ];
        // `movz` zeroes every other bit, so only the chunks that actually carry
        // data need a `movk` after it. Most constants here are small tags and
        // small integers, which is a single instruction instead of four.
        let first = c.iter().position(|x| *x != 0).unwrap_or(0);
        self.word(0xD280_0000 | ((first as u32) << 21) | (c[first] << 5) | rd);
        for (hw, chunk) in c.iter().enumerate().skip(first + 1) {
            if *chunk != 0 {
                self.word(0xF280_0000 | ((hw as u32) << 21) | (chunk << 5) | rd);
            }
        }
    }
    fn ldr(&mut self, rt: u32, rn: u32, byte: u32) {
        self.word(0xF940_0000 | ((byte / 8) << 10) | (rn << 5) | rt);
    }
    fn ldr32(&mut self, rt: u32, rn: u32, byte: u32) {
        debug_assert!(byte % 4 == 0 && byte / 4 < 4096);
        self.word(0xB940_0000 | ((byte / 4) << 10) | (rn << 5) | rt);
    }
    fn str_(&mut self, rt: u32, rn: u32, byte: u32) {
        self.word(0xF900_0000 | ((byte / 8) << 10) | (rn << 5) | rt);
    }
    fn fldr(&mut self, rt: u32, rn: u32, byte: u32) {
        self.word(0xFD40_0000 | ((byte / 8) << 10) | (rn << 5) | rt);
    }
    fn fstr(&mut self, rt: u32, rn: u32, byte: u32) {
        self.word(0xFD00_0000 | ((byte / 8) << 10) | (rn << 5) | rt);
    }
    fn stp_d_off(&mut self, rt: u32, rt2: u32, rn: u32, byte: u32) {
        self.word(0x6D00_0000 | (((byte / 8) & 0x7F) << 15) | (rt2 << 10) | (rn << 5) | rt);
    }
    fn ldp_d_off(&mut self, rt: u32, rt2: u32, rn: u32, byte: u32) {
        self.word(0x6D40_0000 | (((byte / 8) & 0x7F) << 15) | (rt2 << 10) | (rn << 5) | rt);
    }
    #[allow(dead_code)]
    fn adds(&mut self, rd: u32, rn: u32, rm: u32) {
        self.word(0xAB00_0000 | (rm << 16) | (rn << 5) | rd);
    }
    fn cmp(&mut self, rn: u32, rm: u32) {
        self.word(0xEB00_0000 | (rm << 16) | (rn << 5) | 31);
    }
    fn sbfx44(&mut self, rd: u32, rn: u32) {
        self.word(0x9340_0000 | (43 << 10) | (rn << 5) | rd);
    }
    fn ubfx(&mut self, rd: u32, rn: u32, lsb: u32, width: u32) {
        self.word(
            0xD340_0000
                | (((lsb as u32) & 0x3F) << 16)
                | ((((lsb + width - 1) as u32) & 0x3F) << 10)
                | (rn << 5)
                | rd,
        );
    }
    fn orr(&mut self, rd: u32, rn: u32, rm: u32) {
        self.word(0xAA00_0000 | (rm << 16) | (rn << 5) | rd);
    }
    fn scvtf(&mut self, rd: u32, rn: u32) {
        self.word(0x9E62_0000 | (rn << 5) | rd);
    }
    fn fcvtzs(&mut self, rd: u32, rn: u32) {
        self.word(0x9E78_0000 | (rn << 5) | rd);
    }
    fn fmov_d_from_x(&mut self, rd: u32, rn: u32) {
        self.word(0x9E67_0000 | (rn << 5) | rd);
    }
    fn fmov_x_from_d(&mut self, rd: u32, rn: u32) {
        self.word(0x9E66_0000 | (rn << 5) | rd);
    }
    fn fadd(&mut self, rd: u32, rn: u32, rm: u32) {
        self.word(0x1E60_2800 | (rm << 16) | (rn << 5) | rd);
    }
    fn fsub(&mut self, rd: u32, rn: u32, rm: u32) {
        self.word(0x1E60_3800 | (rm << 16) | (rn << 5) | rd);
    }
    fn fmul(&mut self, rd: u32, rn: u32, rm: u32) {
        self.word(0x1E60_0800 | (rm << 16) | (rn << 5) | rd);
    }
    fn fdiv(&mut self, rd: u32, rn: u32, rm: u32) {
        self.word(0x1E60_1800 | (rm << 16) | (rn << 5) | rd);
    }
    fn fcmp(&mut self, rn: u32, rm: u32) {
        self.word(0x1E60_2000 | (rm << 16) | (rn << 5));
    }
    /// `fmov dd, ds` (register to register)
    fn fmov_dd(&mut self, rd: u32, rn: u32) {
        self.word(0x1E60_4000 | (rn << 5) | rd);
    }
    fn eor_regs(&mut self, rd: u32, rn: u32, rm: u32) {
        self.word(0xCA00_0000 | (rm << 16) | (rn << 5) | rd);
    }
    fn cset(&mut self, rd: u32, cc: u32) {
        let inv = cc ^ 1;
        // CSINC Xd, XZR, XZR, inv(cc). Bit 10 is the increment op; leaving it
        // clear encodes CSEL of xzr, xzr, which is always 0.
        self.word(0x9A80_0400 | (31 << 16) | (inv << 12) | (31 << 5) | rd);
    }
    fn b_cond(&mut self, cond: u32, label: usize) {
        let at = self.buf.len();
        self.fixups.push((at, label, 0));
        self.word(0x5400_0000 | cond);
    }
    fn b(&mut self, label: usize) {
        let at = self.buf.len();
        self.fixups.push((at, label, 1));
        self.word(0x1400_0000);
    }
    fn bl(&mut self, label: usize) {
        let at = self.buf.len();
        self.fixups.push((at, label, 1));
        self.word(0x9400_0000);
    }
    fn tbz(&mut self, rt: u32, bit: u32, label: usize) {
        let at = self.buf.len();
        self.fixups.push((at, label, 2));
        self.word(0x3600_0000 | (((bit >> 5) & 1) << 31) | ((bit & 0x1F) << 19) | rt);
    }
    fn tbnz(&mut self, rt: u32, bit: u32, label: usize) {
        let at = self.buf.len();
        self.fixups.push((at, label, 2));
        self.word(0x3700_0000 | (((bit >> 5) & 1) << 31) | ((bit & 0x1F) << 19) | rt);
    }
    fn ret(&mut self) {
        self.word(0xD65F_03C0);
    }
    // ---- additions used by whole-function compilation ----
    fn add_imm(&mut self, rd: u32, rn: u32, imm: u32) {
        self.word(0x9100_0000 | ((imm & 0xFFF) << 10) | (rn << 5) | rd);
    }
    fn sub_imm(&mut self, rd: u32, rn: u32, imm: u32) {
        self.word(0xD100_0000 | ((imm & 0xFFF) << 10) | (rn << 5) | rd);
    }
    fn add_reg(&mut self, rd: u32, rn: u32, rm: u32) {
        self.word(0x8B00_0000 | (rm << 16) | (rn << 5) | rd);
    }
    fn sub_reg(&mut self, rd: u32, rn: u32, rm: u32) {
        self.word(0xCB00_0000 | (rm << 16) | (rn << 5) | rd);
    }
    fn mul_reg(&mut self, rd: u32, rn: u32, rm: u32) {
        self.word(0x9B00_7C00 | (rm << 16) | (rn << 5) | rd);
    }
    fn sdiv(&mut self, rd: u32, rn: u32, rm: u32) {
        self.word(0x9AC0_0C00 | (rm << 16) | (rn << 5) | rd);
    }
    /// `rd = ra - rn * rm`
    fn msub(&mut self, rd: u32, rn: u32, rm: u32, ra: u32) {
        self.word(0x9B00_8000 | (rm << 16) | (ra << 10) | (rn << 5) | rd);
    }
    fn lsr_imm(&mut self, rd: u32, rn: u32, sh: u32) {
        self.word(0xD340_FC00 | ((sh & 0x3F) << 16) | (rn << 5) | rd);
    }
    fn cmp_imm(&mut self, rn: u32, imm: u32) {
        self.word(0xF100_0000 | ((imm & 0xFFF) << 10) | (rn << 5) | 31);
    }
    fn mov_reg(&mut self, rd: u32, rm: u32) {
        self.orr(rd, 31, rm);
    }
    fn stp_pre(&mut self, rt: u32, rt2: u32, imm7: u32) {
        self.word(0xA980_0000 | ((imm7 & 0x7F) << 15) | (rt2 << 10) | (31 << 5) | rt);
    }
    fn ldp_post(&mut self, rt: u32, rt2: u32, imm7: u32) {
        self.word(0xA8C0_0000 | ((imm7 & 0x7F) << 15) | (rt2 << 10) | (31 << 5) | rt);
    }
    fn cbz(&mut self, rt: u32, label: usize) {
        let at = self.buf.len();
        self.fixups.push((at, label, 0));
        self.word(0x3400_0000 | rt);
    }
    fn cbnz(&mut self, rt: u32, label: usize) {
        let at = self.buf.len();
        self.fixups.push((at, label, 0));
        self.word(0x3500_0000 | rt);
    }
    fn blr(&mut self, rn: u32) {
        self.word(0xD63F_0000 | (rn << 5));
    }
    fn csel(&mut self, rd: u32, rn: u32, rm: u32, cond: u32) {
        self.word(0x9A80_0000 | (rm << 16) | ((cond & 0xF) << 12) | (rn << 5) | rd);
    }
}

const R_BASE: u32 = 9;
const R_BOX: u32 = 2;
const R_BBASE: u32 = 13;
const R_TAG: u32 = 15;
const R_ZERO: u32 = 14; // x14 = integer 0
const R_T: u32 = 0;
const R_T2: u32 = 1;
/// The loop body is a leaf — it never calls out — so the whole floating-point
/// register file is available. The callee-saved half (d8..d15) is preserved
/// around the body; the caller-saved half needs no save.
const D_BOUND: u32 = 4;
const D_ZERO: u32 = 5;
const D_SCRATCH: u32 = 6;
const D_SLOTS: [u32; 9] = [7, 8, 9, 10, 11, 12, 13, 14, 15];
/// K constants materialized once before the loop.
const D_KCONST: [u32; 4] = [0, 1, 2, 3];
const X_SLOTS: [u32; 2] = [10, 11];
/// Where spilled literal fields start, past the saved doubles.
const SPILL_BASE: u32 = 64;
/// Object literals a body may materialise to the stack, and the fields each
/// may carry. Four keys is the shape the front end emits for every ordinary
/// `{ a: …, b: …, c: … }`, so four literals is a generous ceiling.
const MAX_LIT: usize = 4;
const MAX_LIT_KEYS: usize = 8;
const FRAME: u32 = SPILL_BASE + (MAX_LIT as u32) * (MAX_LIT_KEYS as u32) * 8;
pub const MAX_BODY: usize = 64;
pub const MAX_SCAN: usize = MAX_BODY + 2;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SlotType {
    Num,
    Bool,
}

enum LoopOp {
    Add {
        dst: u32,
        src: u32,
    },
    AddK {
        dst: u32,
        k: f64,
    },
    Bin {
        op: Op,
        dst: u32,
        a: u32,
        b: u32,
    },
    Cmp {
        op: Op,
        dst: u32,
        a: u32,
        b: u32,
    },
    /// `dst = src <op> k` from `Op::BinOpK`. The constant is loop-invariant and
    /// gets a hoisted register like every other `K` operand.
    BinK {
        op: u8,
        dst: u32,
        src: u32,
        k: f64,
    },
    /// Comparison against a constant from `Op::BinOpK` (payload 5..8).
    CmpK {
        op: u8,
        dst: u32,
        src: u32,
        k: f64,
    },
    JumpIf {
        on_true: bool,
        src: u32,
    },
    LoadNum {
        dst: u32,
        k: f64,
    },
    #[allow(dead_code)]
    LoadBool {
        dst: u32,
        v: bool,
    },
    Move {
        dst: u32,
        src: u32,
    },
    NotBool {
        dst: u32,
        src: u32,
    },
    /// Copy a virtual literal's field values into the loop's scratch frame.
    /// The object itself is never allocated; reads load the value back.
    SpillObj {
        vals: Vec<(u32, SlotType)>,
        off: u32,
    },
    /// `dst = <a spilled field>` — the object's value, loaded from the frame.
    LoadSpill {
        dst: u32,
        off: u32,
        ty: SlotType,
    },
}

fn type_of_slot(types: &[(u32, SlotType)], s: u32) -> Option<SlotType> {
    types.iter().find(|(sl, _)| *sl == s).map(|(_, t)| *t)
}

/// Whether the instruction stores something into register `a`. The
/// read-modify-write shapes (`x += y`) count: they overwrite the register just
/// as plainly as a plain load does. Only the branches leave it alone, and they
/// are the ones the JIT never treats as a destination anyway.
fn overwrites_a(op: Op) -> bool {
    !matches!(
        op,
        Op::Jump
            | Op::JumpIfFalse
            | Op::JumpIfTrue
            | Op::JumpIfNilish
            | Op::JumpIfNotNilish
            | Op::JumpIfNilishKeep
            | Op::JumpLocalCmp
            | Op::PushTry
            | Op::PopTry
            | Op::Nop
    )
}

/// Every control transfer in this bytecode puts its destination in `c`:
/// `Jump`, the four conditional shapes and `JumpLocalCmp` all set `ip = c`,
/// `LocalAddKJ` finishes with `goto c`, and `PushTry` names its catch target
/// there too.
fn jumps_via_c(op: Op) -> bool {
    matches!(
        op,
        Op::Jump
            | Op::JumpIfFalse
            | Op::JumpIfTrue
            | Op::JumpIfNilish
            | Op::JumpIfNotNilish
            | Op::JumpIfNilishKeep
            | Op::JumpLocalCmp
            | Op::LocalAddKJ
            | Op::PushTry
    )
}

/// Whether `a` is a register this instruction simply stores into, without ever
/// looking at what was there before. Everything not listed here is assumed to
/// read `a` — being wrong in that direction only makes the escape check more
/// suspicious, which is the safe way to be wrong.
fn stores_into_a(op: Op) -> bool {
    matches!(
        op,
        Op::Move
            | Op::LoadK
            | Op::LoadUndefined
            | Op::LoadNull
            | Op::LoadTrue
            | Op::LoadFalse
            | Op::LoadThis
            | Op::GetLocal
            | Op::SetLocal
            | Op::GetGlobal
            | Op::GetProp
            | Op::NewObject
            | Op::NewObjectLit
            | Op::NewArray
            | Op::NewArrayLen
            | Op::Add
            | Op::Sub
            | Op::Mul
            | Op::Div
            | Op::Mod
            | Op::Pow
            | Op::Neg
            | Op::TypeOf
            | Op::BitAnd
            | Op::BitOr
            | Op::BitXor
            | Op::Shl
            | Op::Shr
            | Op::UShr
            | Op::BitNot
            | Op::Eq
            | Op::StrictEq
            | Op::Lt
            | Op::Gt
            | Op::Le
            | Op::Ge
            | Op::Not
            | Op::DeleteProp
            | Op::LoadNewTarget
    ) || matches!(op, Op::BinOpK(_))
}

/// A short-lived object literal that the loop body never lets escape.
///
/// `slots` are the registers the literal has been named by — it can be copied
/// around with `SetLocal`, and every name goes stale once the literal becomes
/// virtual. `props` maps each statically known key to the frame slot holding
/// the value stored under it, plus that value's type. A later `o.key` read is
/// then a load from the frame, and the whole literal — allocation, shape
/// transition, property bag — costs nothing but one store per field. The
/// values are *copied* out of the registers, so later reuse of those registers
/// cannot disturb them. `gc_stress` is a pure arithmetic loop with an object in
/// front of it, and this is what makes the loop JIT see it that way.
struct VObj {
    slots: Vec<u32>,
    props: Vec<(PropKey, u32, SlotType)>,
}

/// Compile the counted loop at `ins[top..]`.
/// `ins` is the *whole* function's instruction array and `top` is the
/// absolute index of the loop head, so the scan can also see what comes after
/// the loop.
/// # Safety
/// `ics` must point to the inline-cache array of the function that `ins` was
/// taken from, valid for at least `top + ins.len()` entries for the duration of
/// the call. The VM passes `proto.ics.as_ptr()`.
#[allow(unused_assignments)]
pub unsafe fn try_compile(
    ins: &[Instr],
    top: usize,
    exit_ip: usize,
    consts: &mut dyn FnMut(usize) -> Option<f64>,
    keys: &mut dyn FnMut(usize) -> Option<Vec<PropKey>>,
    ics: *const Ic,
) -> Option<JitLoop> {
    // Native execution is opt-out on supported hosts; all other hosts interpret.
    if !NATIVE_SUPPORTED
        || std::env::var("JHOL_JIT_ENABLE")
            .map(|v| v == "0")
            .unwrap_or(false)
    {
        return None;
    }
    if top >= ins.len() {
        return None;
    }
    let head = &ins[top];
    if head.op != Op::JumpLocalCmp {
        return None;
    }
    let test_slot = head.a;
    let code = (head.b >> 24) as u8;
    if code > 3 {
        return None;
    }
    let bound_kidx = (head.b & 0x00FF_FFFF) as usize;
    let bound = match consts(bound_kidx) {
        Some(v) => v,
        None => {
            if std::env::var_os("JHOL_JIT_DEBUG").is_some() {
                eprintln!("[jit] bound unresolved at kidx={}", bound_kidx);
            }
            return None;
        }
    };
    if bound.fract() != 0.0 || bound.abs() >= SMI_LIMIT as f64 {
        return None;
    }

    // ---- pass 1: validate + type inference ----
    let mut body: Vec<LoopOp> = Vec::new();
    let mut types: Vec<(u32, SlotType)> = vec![(test_slot, SlotType::Num)];
    let mut tail: Option<usize> = None;
    // Object literals that never escape, resolved to their value registers.
    let mut vobjs: Vec<VObj> = Vec::new();
    let mut lit_n: usize = 0;
    let mut i = 1usize; // relative to the head
    let _dbg = std::env::var_os("JHOL_JIT_DEBUG").is_some();
    if _dbg {
        eprintln!("[jit] walk: ins.len()={} i starts at 1", ins.len());
    }
    loop {
        if top + i >= ins.len() || i > MAX_BODY {
            return None;
        }
        let x = &ins[top + i];
        // A slot holding a virtual object never takes part in numeric work; if
        // one is asked for here the loop is doing something with the object
        // that the substitution cannot express, so the whole loop is refused.
        let need_num = |types: &mut Vec<(u32, SlotType)>, vobjs: &[VObj], s: u32| -> Option<()> {
            if vobjs.iter().any(|v| v.slots.contains(&s)) {
                return None;
            }
            if let Some((_, t)) = types.iter().find(|(sl, _)| *sl == s) {
                return if *t == SlotType::Num { Some(()) } else { None };
            }
            types.push((s, SlotType::Num));
            Some(())
        };
        let set_bool = |types: &mut Vec<(u32, SlotType)>, vobjs: &[VObj], s: u32| -> Option<()> {
            if vobjs.iter().any(|v| v.slots.contains(&s)) {
                return None;
            }
            if let Some((_, t)) = types.iter().find(|(sl, _)| *sl == s) {
                return if *t == SlotType::Bool { Some(()) } else { None };
            }
            types.push((s, SlotType::Bool));
            Some(())
        };
        if std::env::var_os("JHOL_JIT_DEBUG").is_some() {
            eprintln!(
                "[jit] walk rel-{}: {:?} a={} b={} c={}",
                i, x.op, x.a, x.b, x.c
            );
        }
        // A register this instruction stores into stops naming whatever
        // virtual object it named: the object itself is untouched, but that
        // register no longer holds it, so a read through that name refuses the
        // loop. This runs *before* the instruction is looked at, because the
        // front end reuses a literal's own destination register as a scratch
        // temp the moment the literal has been copied out of it.
        if overwrites_a(x.op) {
            for v in vobjs.iter_mut() {
                v.slots.retain(|s| *s != x.a);
            }
            vobjs.retain(|v| !v.slots.is_empty());
        }
        match x.op {
            Op::LocalAddKJ => {
                if x.c as usize != top || x.a != test_slot {
                    return None;
                }
                need_num(&mut types, &vobjs, x.a)?;
                // The LocalAddKJ is `slot += K` AND jump — the increment must
                // be pushed as an op or `i` never advances (infinite loop).
                let k = consts(x.b as usize)?;
                body.push(LoopOp::AddK { dst: x.a, k });
                tail = Some(i);
                break;
            }
            Op::LocalAdd => {
                need_num(&mut types, &vobjs, x.a)?;
                need_num(&mut types, &vobjs, x.b)?;
                body.push(LoopOp::Add { dst: x.a, src: x.b });
            }
            Op::LocalAddK => {
                need_num(&mut types, &vobjs, x.a)?;
                let k = consts(x.b as usize)?;
                body.push(LoopOp::AddK { dst: x.a, k });
            }
            Op::Add | Op::Sub | Op::Mul | Op::Div => {
                need_num(&mut types, &vobjs, x.a)?;
                need_num(&mut types, &vobjs, x.b)?;
                need_num(&mut types, &vobjs, x.c)?;
                body.push(LoopOp::Bin {
                    op: x.op,
                    dst: x.a,
                    a: x.b,
                    b: x.c,
                });
            }
            Op::Lt | Op::Gt | Op::Le | Op::Ge | Op::StrictEq | Op::Eq => {
                need_num(&mut types, &vobjs, x.b)?;
                need_num(&mut types, &vobjs, x.c)?;
                set_bool(&mut types, &vobjs, x.a)?;
                body.push(LoopOp::Cmp {
                    op: x.op,
                    dst: x.a,
                    a: x.b,
                    b: x.c,
                });
            }
            Op::JumpIfFalse | Op::JumpIfTrue => {
                if x.c as usize != exit_ip {
                    if std::env::var_os("JHOL_JIT_DEBUG").is_some() {
                        eprintln!(
                            "[jit] reject at rel-{}: {:?} leaves the loop at {} not {}",
                            i, x.op, x.c, exit_ip
                        );
                    }
                    return None;
                }
                // A condition can be a boolean the body computed itself; only
                // an untyped slot needs to be assumed numeric (and the entry
                // guard checks that assumption before the loop runs).
                if type_of_slot(&types, x.a).is_none() {
                    need_num(&mut types, &vobjs, x.a)?;
                }
                body.push(LoopOp::JumpIf {
                    on_true: x.op == Op::JumpIfTrue,
                    src: x.a,
                });
            }
            Op::Not => {
                let src_t = type_of_slot(&types, x.b).unwrap_or(SlotType::Num);
                if src_t != SlotType::Bool {
                    return None;
                }
                set_bool(&mut types, &vobjs, x.a)?;
                body.push(LoopOp::NotBool { dst: x.a, src: x.b });
            }
            Op::LoadK => {
                let v = consts(x.b as usize)?;
                body.push(LoopOp::LoadNum { dst: x.a, k: v });
                need_num(&mut types, &vobjs, x.a)?;
            }
            Op::Move | Op::GetLocal | Op::SetLocal => {
                // Copying a virtual object does not copy the object — it only
                // changes which register names it, so the substitution follows
                // the value instead of materialising anything.
                if let Some(vi) = vobjs.iter().position(|v| v.slots.contains(&x.b)) {
                    if x.a != x.b && type_of_slot(&types, x.a).is_some() {
                        return None;
                    }
                    // The copy adds a name; every other literal loses it.
                    for (j, v) in vobjs.iter_mut().enumerate() {
                        if j == vi {
                            if !v.slots.contains(&x.a) {
                                v.slots.push(x.a);
                            }
                        } else {
                            v.slots.retain(|s| *s != x.a);
                        }
                    }
                    vobjs.retain(|v| !v.slots.is_empty());
                } else {
                    let src_t = type_of_slot(&types, x.b).unwrap_or(SlotType::Num);
                    match src_t {
                        SlotType::Num => need_num(&mut types, &vobjs, x.a)?,
                        SlotType::Bool => set_bool(&mut types, &vobjs, x.a)?,
                    }
                    body.push(LoopOp::Move { dst: x.a, src: x.b });
                }
            }
            // `dst = { k1: v1, … }` with registers `c..c+n` holding the values.
            // The literal becomes a *virtual* object: nothing is allocated, and
            // each field is copied into the loop's own scratch frame, where no
            // later register reuse can disturb it.
            Op::NewObjectLit => {
                let ks = keys(x.b as usize)?;
                if ks.is_empty() || ks.len() > MAX_LIT_KEYS || lit_n >= MAX_LIT {
                    return None;
                }
                let base = x.c;
                // The object slot must not already carry a value, and must not
                // be one of its own value registers.
                if type_of_slot(&types, x.a).is_some() {
                    return None;
                }
                if x.a >= base && (x.a - base) < ks.len() as u32 {
                    return None;
                }
                let off = SPILL_BASE + (lit_n as u32) * (MAX_LIT_KEYS as u32) * 8;
                let mut vals = Vec::with_capacity(ks.len());
                let mut props = Vec::with_capacity(ks.len());
                for (idx, k) in ks.iter().enumerate() {
                    let vs = base + idx as u32;
                    // Every value must already be a tracked number or boolean:
                    // an unwritten register cannot be spilled or read back.
                    let t = match type_of_slot(&types, vs) {
                        Some(t) => t,
                        None => {
                            if std::env::var_os("JHOL_JIT_DEBUG").is_some() {
                                eprintln!("[jit] reject at rel-{}: literal field {} holds an untracked value", i, vs);
                            }
                            return None;
                        }
                    };
                    vals.push((vs, t));
                    props.push((*k, off + (idx as u32) * 8, t));
                }
                body.push(LoopOp::SpillObj { vals, off });
                lit_n += 1;
                for v in vobjs.iter_mut() {
                    v.slots.retain(|s| *s != x.a);
                }
                vobjs.retain(|v| !v.slots.is_empty());
                vobjs.push(VObj {
                    slots: vec![x.a],
                    props,
                });
            }
            // `dst = obj[key]` where `obj` is a virtual literal: `dst` is the
            // field value, read back out of the scratch frame.
            Op::GetProp => {
                let key = unsafe { (*ics.add(top + i)).key };
                let (off, ty) = match vobjs.iter().find(|v| v.slots.contains(&x.b)) {
                    Some(v) => match v.props.iter().find(|(k, _, _)| *k == key) {
                        Some((_, o, t)) => (*o, *t),
                        None => {
                            if std::env::var_os("JHOL_JIT_DEBUG").is_some() {
                                eprintln!("[jit] reject at rel-{}: GetProp key {:?} not in virtual literal (props {:?})", i, key, v.props);
                            }
                            return None;
                        }
                    },
                    None => {
                        if std::env::var_os("JHOL_JIT_DEBUG").is_some() {
                            eprintln!(
                                "[jit] reject at rel-{}: GetProp on non-virtual slot {}",
                                i, x.b
                            );
                        }
                        return None;
                    }
                };
                match ty {
                    SlotType::Num => need_num(&mut types, &vobjs, x.a)?,
                    SlotType::Bool => set_bool(&mut types, &vobjs, x.a)?,
                }
                body.push(LoopOp::LoadSpill { dst: x.a, off, ty });
            }
            Op::BinOpK(sub) => {
                let k = consts(x.c as usize)?;
                match sub {
                    // `+ - * /` against a constant. `i + 1` is how the compiler
                    // emits every constant step, so leaving it out rejected most
                    // real loops before they reached their first interesting op.
                    0..=3 => {
                        need_num(&mut types, &vobjs, x.a)?;
                        need_num(&mut types, &vobjs, x.b)?;
                        body.push(LoopOp::BinK {
                            op: sub,
                            dst: x.a,
                            src: x.b,
                            k,
                        });
                    }
                    // `%` would need a libm `fmod` call and `**` a `pow` call,
                    // neither of which fits a call-free body.
                    4 | 9.. => return None,
                    // `< > <= >=` against a constant: boolean result.
                    _ => {
                        need_num(&mut types, &vobjs, x.b)?;
                        set_bool(&mut types, &vobjs, x.a)?;
                        body.push(LoopOp::CmpK {
                            op: sub,
                            dst: x.a,
                            src: x.b,
                            k,
                        });
                    }
                }
            }
            other => {
                if std::env::var_os("JHOL_JIT_DEBUG").is_some() {
                    eprintln!("[jit] reject at rel-{}: op={:?}", i, other);
                }
                return None;
            }
        }
        i += 1;
    }
    let tail = tail?;

    // A virtual literal is never allocated and never written back, so its
    // registers still hold whatever the interpreter left there when the loop
    // exits. That is invisible only while nothing outside the loop looks at
    // them, so every instruction outside the body is checked for a mention.
    if std::env::var_os("JHOL_JIT_CODE_DUMP").is_some() {
        for (k, y) in ins.iter().enumerate() {
            eprintln!("[code] {:4} {:?} a={} b={} c={}", k, y.op, y.a, y.b, y.c);
        }
    }
    if !vobjs.is_empty() {
        // The walk below forgets what it knew about the registers at every
        // address a branch can land on, and at the two ways out of the loop.
        let mut entry = vec![false; ins.len()];
        for y in ins.iter() {
            if jumps_via_c(y.op) && (y.c as usize) < ins.len() {
                entry[y.c as usize] = true;
            }
        }
        for k in [top + tail + 1, exit_ip] {
            if k < ins.len() {
                entry[k] = true;
            }
        }
        let debug = std::env::var_os("JHOL_JIT_DEBUG").is_some();
        let mut fresh: Vec<u32> = Vec::new();
        for (k, y) in ins.iter().enumerate() {
            if k >= top && k <= top + tail {
                continue;
            }
            if entry[k] {
                fresh.clear();
            }
            let plain = stores_into_a(y.op);
            for v in &vobjs {
                for s in &v.slots {
                    let is_a = *s == y.a;
                    if *s == y.b || *s == y.c || (is_a && !plain) {
                        if !fresh.contains(s) {
                            if debug {
                                eprintln!("[jit] reject: virtual literal register {:?} read stale at ip={} by {:?} a={} b={} c={} (head={} tail={})", v.slots, k, y.op, y.a, y.b, y.c, top, tail);
                            }
                            return None;
                        }
                    } else if is_a && !fresh.contains(s) {
                        fresh.push(*s);
                    }
                }
            }
        }
    }

    if std::env::var_os("JHOL_JIT_DEBUG").is_some() {
        eprintln!(
            "[jit] walk ok, {} body ops, {} slots",
            body.len(),
            types.len()
        );
    }
    // ---- register assignment ----
    let mut d_of: Vec<(u32, u32)> = Vec::new();
    let mut x_of: Vec<(u32, u32)> = Vec::new();
    for (s, t) in &types {
        match t {
            SlotType::Num => {
                if d_of.len() >= D_SLOTS.len() {
                    if std::env::var_os("JHOL_JIT_DEBUG").is_some() {
                        eprintln!(
                            "[jit] reject: {} num slots needed, only {} registers",
                            types.len(),
                            D_SLOTS.len()
                        );
                    }
                    return None;
                }
                d_of.push((*s, D_SLOTS[d_of.len()]));
            }
            SlotType::Bool => {
                if x_of.len() >= X_SLOTS.len() {
                    return None;
                }
                x_of.push((*s, X_SLOTS[x_of.len()]));
            }
        }
    }
    let d_of_slot =
        |s: u32| -> Option<u32> { d_of.iter().find(|(sl, _)| *sl == s).map(|(_, r)| *r) };
    let x_of_slot =
        |s: u32| -> Option<u32> { x_of.iter().find(|(sl, _)| *sl == s).map(|(_, r)| *r) };

    // ---- pass 2: emit ----
    let mut a = Asm::new();
    // Debug: BRK #0 crashes with SIGTRAP if the native code is reached.
    // This confirms the entry path works before debugging the arithmetic.
    if std::env::var_os("JHOL_JIT_BRK").is_some() {
        a.word(0xD420_0000); // brk #0
    }
    // Frame: the callee-saved doubles, then the scratch area virtual literals
    // spill their fields into. Everything below addresses it off `sp`, and both
    // exits tear it down.
    a.sub_imm(31, 31, FRAME);
    a.stp_d_off(8, 9, 31, 0);
    a.stp_d_off(10, 11, 31, 16);
    a.stp_d_off(12, 13, 31, 32);
    a.stp_d_off(14, 15, 31, 48);
    a.orr(R_BASE, 31, 0); // mov x9, x0
    a.mov_const(R_BOX, SMI_BOX);
    a.mov_const(R_BBASE, FALSE_BITS);
    a.mov_const(R_TAG, BOX_TOP);
    a.movz(R_ZERO, 0);
    a.fmov_d_from_x(D_ZERO, R_ZERO); // f11 = 0.0

    let entry_deopt = a.label();
    let done = a.label();
    let loop_top = a.label();

    // ---- entry guard + decode ----
    // Materialize the Smi tag pattern (bits 44..63 == 0x7FF81) in x3 so the
    // per-slot check never needs movz — the old code did `movz x0, #1` INSIDE
    // the guard, which clobbered the loaded value in x0, losing the actual
    // number and producing garbage downstream.
    a.mov_const(3, SMI_TAG20);
    for (slot, t) in &types {
        match t {
            SlotType::Num => {
                let d = d_of_slot(*slot)?;
                a.ldr(R_T, R_BASE, slot * 8);
                // Combined check: top 20 bits must be 0x7FF81 (boxed + Smi).
                a.ubfx(R_T2, R_T, 44, 20);
                a.cmp(R_T2, 3);
                let not_smi = a.label();
                let decoded = a.label();
                a.b_cond(1, not_smi);
                // Boxed Smi: decode the 44-bit payload → f64.
                a.sbfx44(R_T2, R_T);
                a.scvtf(d, R_T2);
                a.b(decoded);
                // Not a boxed Smi: either a raw double ✓ or a non-number ✗.
                a.bind(not_smi);
                a.ubfx(R_T2, R_T, 48, 16);
                a.cmp(R_T2, R_TAG);
                a.b_cond(1, entry_deopt); // boxed but not Smi → deopt
                                          // Unboxed: the raw bits are the f64 value.
                a.fmov_d_from_x(d, R_T);
                a.bind(decoded);
            }
            SlotType::Bool => {
                let x = x_of_slot(*slot)?;
                a.ldr(x, R_BASE, slot * 8);
            }
        }
    }

    // ---- hoist constants: every AddK/LoadNum K is loop-invariant, so
    // materialize them once before the loop instead of 4 movz/movk/fmov
    // inside every iteration. This cuts the counted loop from 14 to 5
    // instructions per iteration.
    let mut kmap: Vec<(u64, u32)> = Vec::new();
    for op in &body {
        match op {
            LoopOp::AddK { k, .. }
            | LoopOp::LoadNum { k, .. }
            | LoopOp::BinK { k, .. }
            | LoopOp::CmpK { k, .. } => {
                let bits = f64::to_bits(*k);
                if !kmap.iter().any(|(b, _)| *b == bits) && kmap.len() < D_KCONST.len() {
                    let dr = D_KCONST[kmap.len()];
                    a.mov_const(R_T, bits);
                    a.fmov_d_from_x(dr, R_T);
                    kmap.push((bits, dr));
                }
            }
            _ => {}
        }
    }
    let kreg = |bits: u64| -> Option<u32> {
        match kmap.iter().find(|(b, _)| *b == bits) {
            Some((_, r)) => Some(*r),
            None => {
                if std::env::var_os("JHOL_JIT_DEBUG").is_some() {
                    eprintln!(
                        "[jit] reject: loop needs more than {} constants at once",
                        D_KCONST.len()
                    );
                }
                None
            }
        }
    };
    // Materialize the loop bound.
    let bound_bits = f64::to_bits(bound);
    a.mov_const(R_T, bound_bits);
    a.fmov_d_from_x(D_BOUND, R_T);

    a.bind(loop_top);
    let d_test = d_of_slot(test_slot)?;
    a.fcmp(d_test, D_BOUND);
    match code {
        0 => a.b_cond(10, done), // <  → exit when >= (GE)
        1 => a.b_cond(13, done), // >  → exit when <= (LE)
        2 => a.b_cond(12, done), // <= → exit when >  (GT)
        _ => a.b_cond(11, done), // >= → exit when <  (LT)
    }

    for op in &body {
        match op {
            LoopOp::Add { dst, src } => {
                a.fadd(d_of_slot(*dst)?, d_of_slot(*dst)?, d_of_slot(*src)?);
            }
            LoopOp::AddK { dst, k } => {
                let dd = d_of_slot(*dst)?;
                let bits = f64::to_bits(*k);
                let dk = kreg(bits)?;
                a.fadd(dd, dd, dk);
            }
            LoopOp::Bin {
                op,
                dst,
                a: l,
                b: r,
            } => {
                let dd = d_of_slot(*dst)?;
                let dl = d_of_slot(*l)?;
                let dr = d_of_slot(*r)?;
                match op {
                    Op::Add => a.fadd(dd, dl, dr),
                    Op::Sub => a.fsub(dd, dl, dr),
                    Op::Mul => a.fmul(dd, dl, dr),
                    Op::Div => a.fdiv(dd, dl, dr),
                    _ => return None,
                }
            }
            LoopOp::Cmp {
                op,
                dst,
                a: l,
                b: r,
            } => {
                let xd = x_of_slot(*dst)?;
                let dl = d_of_slot(*l)?;
                let dr = d_of_slot(*r)?;
                a.fcmp(dl, dr);
                let cc = match op {
                    Op::Lt => 11u32,
                    Op::Gt => 12,
                    Op::Le => 13,
                    Op::Ge => 10,
                    Op::StrictEq | Op::Eq => 0,
                    _ => 1,
                };
                a.cset(xd, cc);
                a.eor_regs(xd, xd, R_BBASE);
            }
            LoopOp::BinK { op, dst, src, k } => {
                let dd = d_of_slot(*dst)?;
                let ds = d_of_slot(*src)?;
                let bits = f64::to_bits(*k);
                let dk = kreg(bits)?;
                match op {
                    0 => a.fadd(dd, ds, dk),
                    1 => a.fsub(dd, ds, dk),
                    2 => a.fmul(dd, ds, dk),
                    3 => a.fdiv(dd, ds, dk),
                    _ => return None,
                }
            }
            LoopOp::CmpK { op, dst, src, k } => {
                let xd = x_of_slot(*dst)?;
                let ds = d_of_slot(*src)?;
                let bits = f64::to_bits(*k);
                let dk = kreg(bits)?;
                // Same condition codes as the register form: every one of these
                // is false on an unordered compare, which is exactly what JS
                // wants from a NaN operand.
                let cc = match op {
                    5 => 11u32, // <
                    6 => 12,    // >
                    7 => 13,    // <=
                    _ => 10,    // >=
                };
                a.fcmp(ds, dk);
                a.cset(xd, cc);
                a.eor_regs(xd, xd, R_BBASE);
            }
            LoopOp::JumpIf { on_true, src } => {
                match type_of_slot(&types, *src) {
                    Some(SlotType::Bool) => {
                        let x = x_of_slot(*src)?;
                        if *on_true {
                            a.tbz(x, 0, done);
                        } else {
                            a.tbnz(x, 0, done);
                        }
                    }
                    _ => {
                        let d = d_of_slot(*src)?;
                        a.fcmp(d, D_ZERO);
                        if *on_true {
                            let is_nan = a.label();
                            a.b_cond(6, is_nan); // VS → NaN → false
                            a.b_cond(1, done); // NE → nonzero → true
                            a.bind(is_nan);
                        } else {
                            a.b_cond(0, done); // EQ → zero
                            a.b_cond(6, done); // VS → NaN
                        }
                    }
                }
            }
            LoopOp::LoadNum { dst, k } => {
                let dd = d_of_slot(*dst)?;
                let bits = f64::to_bits(*k);
                let dk = kreg(bits)?;
                a.fmov_dd(dd, dk);
            }
            LoopOp::LoadBool { dst, v } => {
                let xx = x_of_slot(*dst)?;
                a.mov_const(xx, if *v { TRUE_BITS } else { FALSE_BITS });
            }
            LoopOp::NotBool { dst, src } => {
                let xd = x_of_slot(*dst)?;
                let xs = x_of_slot(*src)?;
                a.movz(R_T, 1);
                a.eor_regs(xd, xs, R_T);
            }
            LoopOp::SpillObj { vals, off } => {
                for (idx, (slot, t)) in vals.iter().enumerate() {
                    let o = off + (idx as u32) * 8;
                    match t {
                        SlotType::Num => a.fstr(d_of_slot(*slot)?, 31, o),
                        SlotType::Bool => a.str_(x_of_slot(*slot)?, 31, o),
                    }
                }
            }
            LoopOp::LoadSpill { dst, off, ty } => match ty {
                SlotType::Num => a.fldr(d_of_slot(*dst)?, 31, *off),
                SlotType::Bool => a.ldr(x_of_slot(*dst)?, 31, *off),
            },
            LoopOp::Move { dst, src } => {
                if let (Some(dd), Some(ds)) = (d_of_slot(*dst), d_of_slot(*src)) {
                    a.fmov_dd(dd, ds);
                } else if let (Some(xd), Some(xs)) = (x_of_slot(*dst), x_of_slot(*src)) {
                    a.orr(xd, 31, xs);
                }
            }
        }
    }

    a.b(loop_top);

    // ---- done: encode every slot back ----
    a.bind(done);
    for (slot, t) in &types {
        match t {
            SlotType::Num => {
                let d = d_of_slot(*slot)?;
                // NaN check
                a.fcmp(d, d);
                let nan_store = a.label();
                let next = a.label();
                a.b_cond(6, nan_store);
                // integral check: fcvtzs + round-trip
                a.fcvtzs(R_T, d);
                a.scvtf(D_SCRATCH, R_T);
                a.fcmp(d, D_SCRATCH);
                let f64_store = a.label();
                a.b_cond(1, f64_store);
                // IEEE -0 is observable through division and Object.is.
                a.fmov_x_from_d(R_T2, d);
                let nonzero = a.label();
                a.cbnz(R_T, nonzero);
                a.tbnz(R_T2, 63, f64_store);
                a.bind(nonzero);
                // range check: |val| < 2^43
                a.sbfx44(R_T2, R_T);
                a.cmp(R_T2, R_T);
                a.b_cond(1, f64_store);
                // Smi store
                a.orr(R_T, R_T, R_BOX);
                a.str_(R_T, R_BASE, slot * 8);
                a.b(next);
                // f64 store
                a.bind(f64_store);
                a.fmov_x_from_d(R_T, d);
                a.str_(R_T, R_BASE, slot * 8);
                a.b(next);
                // NaN store
                a.bind(nan_store);
                a.mov_const(R_T, NAN_BITS);
                a.str_(R_T, R_BASE, slot * 8);
                a.bind(next);
            }
            SlotType::Bool => {
                let x = x_of_slot(*slot)?;
                a.str_(x, R_BASE, slot * 8);
            }
        }
    }
    a.ldp_d_off(8, 9, 31, 0);
    a.ldp_d_off(10, 11, 31, 16);
    a.ldp_d_off(12, 13, 31, 32);
    a.ldp_d_off(14, 15, 31, 48);
    a.add_imm(31, 31, FRAME);
    a.movz(R_T, 1);
    a.ret();

    a.bind(entry_deopt);
    a.ldp_d_off(8, 9, 31, 0);
    a.ldp_d_off(10, 11, 31, 16);
    a.ldp_d_off(12, 13, 31, 32);
    a.ldp_d_off(14, 15, 31, 48);
    a.add_imm(31, 31, FRAME);
    a.movz(R_T, 0);
    a.ret();

    if std::env::var_os("JHOL_JIT_DEBUG").is_some() {
        eprintln!("[jit] emission done: {} instructions", a.buf.len());
    }
    let buf = a.finish();
    let byte_len = buf.len() * 4;
    if let Some(dump) = std::env::var_os("JHOL_JIT_DUMP") {
        let bytes: Vec<u8> = buf.iter().flat_map(|w| w.to_le_bytes()).collect();
        let _ = std::fs::write(&dump, bytes);
    }
    let code_mem = alloc_exec(byte_len);
    if code_mem.is_null() {
        return None;
    }
    for (n, w) in buf.iter().enumerate() {
        unsafe { (code_mem as *mut u32).add(n).write(*w) };
    }
    if !seal_exec(code_mem, byte_len) {
        unsafe { release_exec(code_mem, byte_len) };
        return None;
    }
    Some(JitLoop {
        code: code_mem,
        len: byte_len,
        exit_ip,
    })
}

/// Run compiled loop code directly. Returns `true` when the loop completed
/// (the VM continues after it) and `false` when it deoptimized.
pub unsafe fn run_raw(code: *const u8, regs: *mut Value, depth: u32) -> u32 {
    let f: unsafe extern "C" fn(*mut Value, u32) -> u32 = std::mem::transmute(code);
    f(regs, depth)
}

// ---------------------------------------------------------------------------
// Whole-function native compilation
//
// The counted-loop JIT above removes the dispatch loop from one loop but still
// enters and leaves through the interpreter. This compiles a *whole function*
// — the hot leaf functions that dominate call-heavy code — into a native
// routine whose register file lives in its own stack frame. The interpreter
// calls it directly, so a call stops costing a `Frame` push, a frame-cache
// rebuild, an instruction-fetch loop and a return trip back out.
//
// Everything is a guard that deoptimizes to the interpreter, and the whole
// native call tree is side-effect free, so re-running a deoptimized call is
// always safe:
//
//   * every value computed is an `Smi` or a boolean; any other operand
//     deoptimizes the call, which then runs interpreted and produces whatever
//     JavaScript actually requires
//   * no property access, no allocation, no strings, no generators, and a
//     call may only reach another compiled function
//   * a function that writes an upvalue may not contain a call at all, and may
//     not deoptimize after its first write — so a deoptimized call never
//     repeats a side effect
//
// Slot values stay boxed in memory (`Value`s in the native frame), so the
// collector sees exactly what it would have seen in the interpreter's register
// window; only unboxed temporaries live in registers, and a function that
// qualifies allocates nothing, so no collection can happen mid-call.
// ---------------------------------------------------------------------------

use std::sync::atomic::Ordering;

use crate::bytecode::{ConstVal, FnProto};
use crate::objects::{FnKind, JCell};

/// Smi payload mask.
const SMI_MASK: u64 = 0x0000_0FFF_FFFF_FFFF;
/// Smi tag word, in the engine's own encoding.
const SMI_TOP20: u64 = 0x7FF81;
const V_UNDEF: u64 = Value::UNDEFINED.0;
const V_NULL: u64 = Value::NULL.0;
const V_TRUE: u64 = Value::TRUE.0;
const V_FALSE: u64 = Value::FALSE.0;
/// The interpreter's frame limit. A native call that would exceed it returns
/// `1` so the interpreter re-runs the call and raises the usual `RangeError`.
const NATIVE_MAX_FRAMES: u64 = 900;
const NATIVE_MAX_SLOTS: usize = 48;
const NATIVE_MAX_OPS: usize = 192;
const NATIVE_POOL: usize = 5;

/// Byte offset of the value inside a `JCell`. The cell is an ordinary heap
/// object (`hdr` first, then `v`), so a cell read is a load at this offset —
/// **not** at zero, which would yield the object header.
const CELL_VALUE_OFF: u32 = std::mem::offset_of!(crate::objects::JCell, v) as u32;
/// Interpreted calls a function takes before it is compiled.
pub const NATIVE_CALL_AFTER: u32 = 48;
/// Deopts after compilation before the compiled code is abandoned.
pub const NATIVE_DEOPT_LIMIT: u32 = 6;

/// Saved registers: x19..x28 (the pool lives at the top of that range).
const X_DEPTH: u32 = 19;
const X_OUT: u32 = 20;
const X_UPVALS: u32 = 21;
const X_SMI: u32 = 22;
const X_HELP: u32 = 23;
const X_K0: u32 = 24;
/// Scratch.
const X_T0: u32 = 9;
const X_T1: u32 = 10;
const X_T2: u32 = 11;
const X_T3: u32 = 12;
const X_T4: u32 = 13;
const X_SP: u32 = 31;
/// `stp`/`ldp` imm7 for a 16-byte pair (signed offset / 8).
const NEG16: u32 = 0x7E;
const POS16: u32 = 2;

/// The native entry point of a compiled function:
/// `(args, argc, depth, out, upvalues) -> status`, `0` = ok, `1` = deopt.
type NativeEntry =
    extern "C" fn(*const Value, u32, u32, *mut Value, *const *mut JCell, u64, u64) -> u32;

/// Lowered instruction. Jump targets are instruction indices.
#[derive(Debug)]
enum NReg {
    /// `slot[dst] = slot[src]`
    Copy {
        dst: u32,
        src: u32,
    },
    /// `slot[dst] = bits`
    Bits {
        dst: u32,
        bits: u64,
    },
    /// `slot[dst] = *upvals[up]`
    Upval {
        dst: u32,
        up: u32,
    },
    /// `*upvals[up] = slot[src]`
    StoreUpval {
        up: u32,
        src: u32,
    },
    /// `slot[dst] = slot[l] <op> slot[r]` — 0 add, 1 sub, 2 mul, 3 mod
    Arith {
        op: u8,
        dst: u32,
        l: u32,
        r: u32,
    },
    /// `slot[dst] = slot[l] <op> pool[k]`
    ArithK {
        op: u8,
        dst: u32,
        l: u32,
        k: usize,
    },
    /// comparison — 0 lt, 1 gt, 2 le, 3 ge
    Cmp {
        op: u8,
        dst: u32,
        l: u32,
        r: u32,
        kt: usize,
        kf: usize,
    },
    CmpK {
        op: u8,
        dst: u32,
        l: u32,
        k: usize,
        kt: usize,
        kf: usize,
    },
    /// `===` on two Smis
    Eq {
        dst: u32,
        l: u32,
        r: u32,
        kt: usize,
        kf: usize,
    },
    Jump {
        t: usize,
    },
    Branch {
        on_true: bool,
        src: u32,
        t: usize,
        kt: usize,
    },
    /// `this_slot` is the receiver slot for `CallThis`; `None` calls the
    /// callee with `undefined`, which the pool carries as entry `ku`.
    Invoke {
        dst: u32,
        callee: u32,
        arg0: u32,
        argc: u32,
        this_slot: Option<u32>,
        ku: usize,
    },
    Ret {
        src: u32,
    },
    Nop,
}

struct NPlan {
    ir: Vec<NReg>,
    pool: Vec<u64>,
}

/// Smi bit pattern for an integral constant, or `None` when the constant is
/// not exactly an in-range integer (`-0` included: it has its own identity).
fn smi_bits_int(n: f64) -> Option<u64> {
    if !n.is_finite() || n.fract() != 0.0 {
        return None;
    }
    if n == 0.0 && n.is_sign_negative() {
        return None;
    }
    if n < -(1i64 << 43) as f64 || n >= (1i64 << 43) as f64 {
        return None;
    }
    Some(SMI_BOX_VAL | ((n as i64 as u64) & SMI_MASK))
}

const SMI_BOX_VAL: u64 = 0x7FF8_1000_0000_0000;

/// Is `bits` a boxed Smi?
fn bits_is_smi(bits: u64) -> bool {
    bits >> 44 == SMI_TOP20
}

/// Operators that can never deoptimize: everything else must not follow a
/// `SetUpval`, or a re-run would repeat the write.
fn no_deopt_after_setupval(op: Op) -> bool {
    matches!(
        op,
        Op::Move
            | Op::GetLocal
            | Op::SetLocal
            | Op::LoadK
            | Op::LoadUndefined
            | Op::LoadNull
            | Op::LoadTrue
            | Op::LoadFalse
            | Op::GetUpval
            | Op::SetUpval
            | Op::Jump
            | Op::JumpIfFalse
            | Op::JumpIfTrue
            | Op::Return
            | Op::Nop
    )
}

fn lower(proto: &FnProto) -> Option<NPlan> {
    if proto.is_generator || proto.nslots == 0 || proto.nslots > NATIVE_MAX_SLOTS {
        return None;
    }
    let code = &proto.code;
    if code.is_empty() || code.len() > NATIVE_MAX_OPS {
        return None;
    }
    let is_arrow = proto.is_arrow;
    let nslots = proto.nslots as u32;
    let _param_base = if is_arrow { 0u32 } else { 2u32 };
    let _nparams = proto.nparams as u32;
    let n = code.len();

    let mut ir: Vec<NReg> = Vec::with_capacity(n);
    let mut pool: Vec<u64> = Vec::new();
    let mut seen_setupval = false;
    let mut has_call = false;
    let mut has_setupval = false;

    // `want` dedups constants; the pool is bounded by callee-saved registers.
    fn want(pool: &mut Vec<u64>, bits: u64) -> Option<usize> {
        if let Some(i) = pool.iter().position(|b| *b == bits) {
            return Some(i);
        }
        if pool.len() >= NATIVE_POOL {
            return None;
        }
        pool.push(bits);
        Some(pool.len() - 1)
    }

    for (_ip, ins) in code.iter().enumerate() {
        let op = ins.op;
        let (a, b, c) = (ins.a, ins.b, ins.c);
        // A non-arrow function's slot 0 holds `this`, which the entry carries
        // in x5 and stores into the frame; slot 1 is `new.target`, which is
        // always `undefined` here because constructors never run natively
        // (`construct` enters a frame directly, bypassing this path). Both are
        // ordinary, untyped slots as far as the rest of the compiler cares.
        let chk = |s: u32| -> Option<()> {
            if s >= nslots {
                return None;
            }
            Some(())
        };
        let r = match op {
            Op::Nop => NReg::Nop,
            Op::Move | Op::GetLocal | Op::SetLocal => {
                chk(b)?;
                chk(a)?;
                NReg::Copy { dst: a, src: b }
            }
            Op::LoadUndefined => NReg::Bits {
                dst: a,
                bits: V_UNDEF,
            },
            Op::LoadNull => NReg::Bits {
                dst: a,
                bits: V_NULL,
            },
            Op::LoadTrue => NReg::Bits {
                dst: a,
                bits: V_TRUE,
            },
            Op::LoadFalse => NReg::Bits {
                dst: a,
                bits: V_FALSE,
            },
            Op::LoadK => {
                let k = proto.consts.get(b as usize)?;
                let bits = match k {
                    ConstVal::Num(v) => smi_bits_int(*v)?,
                    ConstVal::True => V_TRUE,
                    ConstVal::False => V_FALSE,
                    ConstVal::Undefined => V_UNDEF,
                    ConstVal::Null => V_NULL,
                    _ => return None,
                };
                NReg::Bits { dst: a, bits }
            }
            Op::GetUpval => {
                if b >= proto.nupvals as u32 {
                    return None;
                }
                NReg::Upval { dst: a, up: b }
            }
            Op::SetUpval => {
                if a >= proto.nupvals as u32 {
                    return None;
                }
                has_setupval = true;
                seen_setupval = true;
                NReg::StoreUpval { up: a, src: b }
            }
            Op::Add | Op::Sub | Op::Mul | Op::Mod => {
                chk(b)?;
                chk(c)?;
                let o = match op {
                    Op::Add => 0u8,
                    Op::Sub => 1,
                    Op::Mul => 2,
                    _ => 3,
                };
                NReg::Arith {
                    op: o,
                    dst: a,
                    l: b,
                    r: c,
                }
            }
            Op::BinOpK(sub) => {
                chk(b)?;
                let k = proto.consts.get(c as usize)?;
                let bits = match k {
                    ConstVal::Num(v) => smi_bits_int(*v)?,
                    _ => return None,
                };
                // Arithmetic/compare operands are consumed *unboxed*, so the
                // pool has to hold the sign-extended integer, not the Smi
                // pattern `LoadK` stores into a slot.
                let bits = (((bits & SMI_MASK) << 20) as i64 >> 20) as u64;
                if sub <= 2 || sub == 4 {
                    let o = if sub == 4 { 3u8 } else { sub };
                    let ki = want(&mut pool, bits)?;
                    NReg::ArithK {
                        op: o,
                        dst: a,
                        l: b,
                        k: ki,
                    }
                } else if sub >= 5 && sub <= 8 {
                    let ki = want(&mut pool, bits)?;
                    let kt = want(&mut pool, V_TRUE)?;
                    let kf = want(&mut pool, V_FALSE)?;
                    NReg::CmpK {
                        op: sub - 5,
                        dst: a,
                        l: b,
                        k: ki,
                        kt,
                        kf,
                    }
                } else {
                    return None;
                }
            }
            Op::Lt | Op::Gt | Op::Le | Op::Ge => {
                chk(b)?;
                chk(c)?;
                let o = match op {
                    Op::Lt => 0u8,
                    Op::Gt => 1,
                    Op::Le => 2,
                    _ => 3,
                };
                let kt = want(&mut pool, V_TRUE)?;
                let kf = want(&mut pool, V_FALSE)?;
                NReg::Cmp {
                    op: o,
                    dst: a,
                    l: b,
                    r: c,
                    kt,
                    kf,
                }
            }
            Op::StrictEq => {
                chk(b)?;
                chk(c)?;
                let kt = want(&mut pool, V_TRUE)?;
                let kf = want(&mut pool, V_FALSE)?;
                NReg::Eq {
                    dst: a,
                    l: b,
                    r: c,
                    kt,
                    kf,
                }
            }
            Op::Jump => {
                if c as usize > n {
                    return None;
                }
                NReg::Jump { t: c as usize }
            }
            Op::JumpIfFalse | Op::JumpIfTrue => {
                chk(a)?;
                if c as usize > n {
                    return None;
                }
                let kt = want(&mut pool, V_TRUE)?;
                NReg::Branch {
                    on_true: op == Op::JumpIfTrue,
                    src: a,
                    t: c as usize,
                    kt,
                }
            }
            Op::Call | Op::CallThis => {
                has_call = true;
                let (callee, arg0) = if op == Op::Call {
                    (b, b + 1)
                } else {
                    (b + 1, b + 2)
                };
                chk(callee)?;
                for i in 0..c {
                    chk(arg0 + i)?;
                }
                // `CallThis` forwards its receiver slot; a plain `Call` passes
                // `undefined`, which the pool carries for the whole function.
                let ku = want(&mut pool, V_UNDEF)?;
                let this_slot = if op == Op::CallThis {
                    chk(b)?;
                    Some(b)
                } else {
                    None
                };
                NReg::Invoke {
                    dst: a,
                    callee,
                    arg0,
                    argc: c,
                    this_slot,
                    ku,
                }
            }
            Op::Return => {
                chk(a)?;
                NReg::Ret { src: a }
            }
            _ => return None,
        };
        if seen_setupval && !no_deopt_after_setupval(op) {
            return None;
        }
        ir.push(r);
    }
    // A call in the same function as an upvalue write means a native caller
    // could deoptimize after the write had already happened.
    if has_call && has_setupval {
        return None;
    }
    // Every path has to end somewhere; a fall-off-the-end is a `return
    // undefined`, which the epilogue provides. Nothing else to check.
    Some(NPlan { ir, pool })
}

/// Forward, must-style dataflow over slot kinds: `0` unknown, `1` `Smi`, `2`
/// boolean. A `Smi` claim lets an arithmetic operand skip its runtime tag test,
/// and a boolean claim turns a branch into a single bit test. Nodes are
/// processed from a worklist seeded at entry, so a predecessor that is never
/// reached can never contribute a claim.
fn kind_of_slots(plan: &NPlan, nslots: usize, base: &[u8]) -> Vec<u8> {
    let n = plan.ir.len();
    let mut kb = vec![0u8; (n + 1) * nslots];
    let mut seen = vec![false; n + 1];
    let mut queued = vec![false; n + 1];
    kb[..nslots].copy_from_slice(base);
    seen[0] = true;
    queued[0] = true;
    let mut work: Vec<usize> = vec![0];
    let mut succ: [usize; 2] = [0, 0];
    if std::env::var_os("JHOL_JIT_DEBUG").is_some() {
        eprintln!("[jit] plan: {} ops, {} nslots", n, nslots);
    }
    while let Some(ip) = work.pop() {
        queued[ip] = false;
        let mut out = kb[ip * nslots..(ip + 1) * nslots].to_vec();
        match plan.ir[ip] {
            NReg::Copy { dst, src } => out[dst as usize] = out[src as usize],
            NReg::Bits { dst, bits } => {
                out[dst as usize] = if bits_is_smi(bits) {
                    1
                } else if bits == V_TRUE || bits == V_FALSE {
                    2
                } else {
                    0
                };
            }
            NReg::Arith { dst, .. } | NReg::ArithK { dst, .. } => out[dst as usize] = 1,
            NReg::Cmp { dst, .. } | NReg::CmpK { dst, .. } | NReg::Eq { dst, .. } => {
                out[dst as usize] = 2
            }
            NReg::Upval { dst, .. } | NReg::Invoke { dst, .. } => out[dst as usize] = 0,
            _ => {}
        }
        let ns = match plan.ir[ip] {
            NReg::Ret { .. } => 0,
            NReg::Jump { t } => {
                succ[0] = t;
                1
            }
            NReg::Branch { t, .. } => {
                succ[0] = t;
                succ[1] = ip + 1;
                2
            }
            _ => {
                succ[0] = ip + 1;
                1
            }
        };
        for k in 0..ns {
            let t = succ[k];
            let changed = if !seen[t] {
                seen[t] = true;
                kb[t * nslots..(t + 1) * nslots].copy_from_slice(&out);
                true
            } else {
                let mut dirty = false;
                for s in 0..nslots {
                    let a = kb[t * nslots + s];
                    let b = out[s];
                    let m = if a == b { a } else { 0 };
                    if m != a {
                        kb[t * nslots + s] = m;
                        dirty = true;
                    }
                }
                dirty
            };
            if changed && !queued[t] {
                queued[t] = true;
                work.push(t);
            }
        }
    }
    kb
}
struct NEmit {
    a: Asm,
    #[allow(dead_code)]
    pool: Vec<u64>,
    deopt: usize,
    ret_ok: usize,
    kind: Vec<u8>,
}

impl NEmit {
    fn mul_checked(&mut self, dst: u32, l: u32, r: u32) {
        self.a.mul_reg(dst, l, r);
        self.a.word(0x9b407c00 | (r << 16) | (l << 5) | X_T3); // smulh
        self.a.word(0x937ffc00 | (dst << 5) | X_T4); // asr #63
        self.a.cmp(X_T3, X_T4);
        self.a.b_cond(1, self.deopt);
        let nonzero = self.a.label();
        self.a.cbnz(dst, nonzero);
        self.a.eor_regs(X_T3, l, r);
        self.a.tbnz(X_T3, 63, self.deopt);
        self.a.bind(nonzero);
    }

    fn kreg(&self, i: usize) -> u32 {
        X_K0 + i as u32
    }
    /// Load `slot`, unboxed. Skips the tag test when the slot is known `Smi`.
    fn unbox(&mut self, slot: u32, out: u32) {
        let known = self.kind.get(slot as usize).copied().unwrap_or(0) == 1;
        self.a.ldr(out, X_SP, slot * 8);
        if !known {
            let d = self.deopt;
            self.a.sub_reg(X_T3, out, X_SMI);
            self.a.lsr_imm(X_T3, X_T3, 44);
            self.a.cbnz(X_T3, d);
        }
        self.a.sbfx44(out, out);
    }
    /// Range-check `v` against the Smi space and box it in place.
    fn boxv(&mut self, v: u32) {
        let d = self.deopt;
        self.a.sbfx44(X_T3, v);
        self.a.cmp(X_T3, v);
        self.a.b_cond(1, d);
        self.a.ubfx(v, v, 0, 44);
        self.a.orr(v, v, X_SMI);
    }
    fn store(&mut self, slot: u32, reg: u32) {
        self.a.str_(reg, X_SP, slot * 8);
    }
}

fn emit_func(
    plan: &NPlan,
    kinds: &[u8],
    nslots: u32,
    param_base: u32,
    nparams: u32,
) -> Option<Vec<u32>> {
    let n = plan.ir.len();
    let frame = (((nslots * 8) + 8) + 15) & !15;
    let mut e = NEmit {
        a: Asm::new(),
        pool: plan.pool.clone(),
        deopt: 0,
        ret_ok: 0,
        kind: Vec::new(),
    };
    let labels: Vec<usize> = (0..=n).map(|_| e.a.label()).collect();
    let ret_ok = e.a.label();
    let deopt = e.a.label();
    let epi = e.a.label();
    e.deopt = deopt;
    e.ret_ok = ret_ok;

    // ---- prologue ----
    e.a.stp_pre(X_DEPTH, X_OUT, NEG16);
    e.a.stp_pre(X_UPVALS, X_SMI, NEG16);
    e.a.stp_pre(X_HELP, X_K0, NEG16);
    e.a.stp_pre(X_K0 + 1, X_K0 + 2, NEG16);
    e.a.stp_pre(X_K0 + 3, X_K0 + 4, NEG16);
    e.a.sub_imm(X_SP, X_SP, frame);
    // The function is entered with `blr`, so LR holds our own return address.
    // Any `blr` in the body (the native invoke helper) overwrites it, so it has
    // to live in the frame.
    e.a.str_(30, X_SP, nslots * 8);
    // depth guard, on the incoming register (after the frame, so a deopt here
    // unwinds with the same `add sp, sp, #frame` as every other deopt)
    e.a.mov_const(X_T0, NATIVE_MAX_FRAMES);
    e.a.cmp(2, X_T0);
    e.a.b_cond(2, deopt); // HS
    e.a.mov_reg(X_DEPTH, 2);
    e.a.mov_reg(X_OUT, 3);
    e.a.mov_reg(X_UPVALS, 4);
    e.a.mov_const(X_SMI, SMI_BOX_VAL);
    let helper = jhol_native_invoke
        as unsafe extern "C" fn(u64, u64, *const Value, u32, u32, *mut Value) -> u32
        as usize as u64;
    e.a.mov_const(X_HELP, helper);
    for (i, bits) in plan.pool.iter().enumerate() {
        e.a.mov_const(e.kreg(i), *bits);
    }
    // slots start as `undefined`, exactly as a frame entry leaves them
    e.a.mov_const(X_T0, V_UNDEF);
    for i in 0..nslots {
        e.a.str_(X_T0, X_SP, i * 8);
    }
    // Slot 0 is `this` for a non-arrow function (x5; not clobbered above).
    if param_base != 0 {
        e.a.str_(5, X_SP, 0);
    }
    // arity guard + argument copy + tag guard
    if nparams > 0 {
        e.a.mov_const(X_T0, nparams as u64);
        e.a.cmp(1, X_T0);
        e.a.b_cond(3, deopt); // LO: fewer arguments than parameters
    }
    for i in 0..nparams {
        e.a.ldr(X_T1, 0, i * 8);
        e.a.sub_reg(X_T2, X_T1, X_SMI);
        e.a.lsr_imm(X_T2, X_T2, 44);
        e.a.cbnz(X_T2, deopt);
        e.a.str_(X_T1, X_SP, (param_base + i) * 8);
    }

    // ---- body ----
    for ip in 0..n {
        e.a.bind(labels[ip]);
        e.kind = kinds[ip * nslots as usize..(ip + 1) * nslots as usize].to_vec();
        match plan.ir[ip] {
            NReg::Nop => {}
            NReg::Copy { dst, src } => {
                if dst != src {
                    e.a.ldr(X_T0, X_SP, src * 8);
                    e.a.str_(X_T0, X_SP, dst * 8);
                }
            }
            NReg::Bits { dst, bits } => {
                e.a.mov_const(X_T0, bits);
                e.a.str_(X_T0, X_SP, dst * 8);
            }
            NReg::Upval { dst, up } => {
                e.a.ldr(X_T0, X_UPVALS, up * 8);
                e.a.ldr(X_T0, X_T0, CELL_VALUE_OFF);
                e.a.str_(X_T0, X_SP, dst * 8);
            }
            NReg::StoreUpval { up, src } => {
                e.a.ldr(X_T0, X_UPVALS, up * 8);
                e.a.ldr(X_T1, X_SP, src * 8);
                e.a.str_(X_T1, X_T0, CELL_VALUE_OFF);
            }
            NReg::Arith { op, dst, l, r } => {
                e.unbox(l, X_T0);
                e.unbox(r, X_T1);
                match op {
                    0 => e.a.add_reg(X_T2, X_T0, X_T1),
                    1 => e.a.sub_reg(X_T2, X_T0, X_T1),
                    2 => e.mul_checked(X_T2, X_T0, X_T1),
                    _ => {
                        // `%`: a zero divisor has to go through the interpreter
                        // (it yields NaN, not a trap).
                        e.a.cmp_imm(X_T1, 0);
                        let d = e.deopt;
                        e.a.b_cond(0, d);
                        e.a.sdiv(X_T3, X_T0, X_T1);
                        e.a.msub(X_T2, X_T3, X_T1, X_T0);
                        let nonzero = e.a.label();
                        e.a.cbnz(X_T2, nonzero);
                        e.a.tbnz(X_T0, 63, e.deopt);
                        e.a.bind(nonzero);
                    }
                }
                e.boxv(X_T2);
                e.store(dst, X_T2);
            }
            NReg::ArithK { op, dst, l, k } => {
                e.unbox(l, X_T0);
                let kr = e.kreg(k);
                match op {
                    0 => e.a.add_reg(X_T2, X_T0, kr),
                    1 => e.a.sub_reg(X_T2, X_T0, kr),
                    2 => e.mul_checked(X_T2, X_T0, kr),
                    _ => {
                        e.a.cmp_imm(kr, 0);
                        let d = e.deopt;
                        e.a.b_cond(0, d);
                        e.a.sdiv(X_T3, X_T0, kr);
                        e.a.msub(X_T2, X_T3, kr, X_T0);
                        let nonzero = e.a.label();
                        e.a.cbnz(X_T2, nonzero);
                        e.a.tbnz(X_T0, 63, e.deopt);
                        e.a.bind(nonzero);
                    }
                }
                e.boxv(X_T2);
                e.store(dst, X_T2);
            }
            NReg::Cmp {
                op,
                dst,
                l,
                r,
                kt,
                kf,
            } => {
                e.unbox(l, X_T0);
                e.unbox(r, X_T1);
                e.a.cmp(X_T0, X_T1);
                let cc = match op {
                    0 => 11u32,
                    1 => 12,
                    2 => 13,
                    _ => 10,
                };
                let (ktr, kfr) = (e.kreg(kt), e.kreg(kf));
                e.a.csel(X_T2, ktr, kfr, cc);
                e.store(dst, X_T2);
            }
            NReg::CmpK {
                op,
                dst,
                l,
                k,
                kt,
                kf,
            } => {
                e.unbox(l, X_T0);
                let kr = e.kreg(k);
                e.a.cmp(X_T0, kr);
                let cc = match op {
                    0 => 11u32,
                    1 => 12,
                    2 => 13,
                    _ => 10,
                };
                let (ktr, kfr) = (e.kreg(kt), e.kreg(kf));
                e.a.csel(X_T2, ktr, kfr, cc);
                e.store(dst, X_T2);
            }
            NReg::Eq { dst, l, r, kt, kf } => {
                e.unbox(l, X_T0);
                e.unbox(r, X_T1);
                e.a.cmp(X_T0, X_T1);
                let (ktr, kfr) = (e.kreg(kt), e.kreg(kf));
                e.a.csel(X_T2, ktr, kfr, 0);
                e.store(dst, X_T2);
            }
            NReg::Jump { t } => {
                e.a.b(labels[t]);
            }
            NReg::Branch {
                on_true,
                src,
                t,
                kt,
            } => {
                let kind = e.kind.get(src as usize).copied().unwrap_or(0);
                e.a.ldr(X_T0, X_SP, src * 8);
                match kind {
                    1 => {
                        e.a.sbfx44(X_T1, X_T0);
                        if on_true {
                            e.a.cbnz(X_T1, labels[t]);
                        } else {
                            e.a.cbz(X_T1, labels[t]);
                        }
                    }
                    2 => {
                        let ktr = e.kreg(kt);
                        e.a.cmp(X_T0, ktr);
                        e.a.b_cond(if on_true { 0 } else { 1 }, labels[t]);
                    }
                    _ => return None,
                }
            }
            NReg::Invoke {
                dst,
                callee,
                arg0,
                argc,
                this_slot,
                ku,
            } => {
                e.a.ldr(0, X_SP, callee * 8);
                match this_slot {
                    Some(s) => e.a.ldr(1, X_SP, s * 8),
                    None => e.a.mov_reg(1, e.kreg(ku)),
                }
                e.a.add_imm(2, X_SP, arg0 * 8);
                e.a.movz(3, argc as u64);
                e.a.mov_reg(4, X_DEPTH);
                e.a.add_imm(5, X_SP, dst * 8);
                e.a.blr(X_HELP);
                let d = e.deopt;
                e.a.cbnz(0, d);
            }
            NReg::Ret { src } => {
                e.a.ldr(X_T0, X_SP, src * 8);
                e.a.str_(X_T0, X_OUT, 0);
                let r = e.ret_ok;
                e.a.b(r);
            }
        }
    }

    // ---- fall-off-the-end returns undefined ----
    e.a.bind(labels[n]);
    e.a.mov_const(X_T0, V_UNDEF);
    e.a.str_(X_T0, X_OUT, 0);
    let r = e.ret_ok;
    e.a.b(r);

    e.a.bind(ret_ok);
    e.a.movz(0, 0);
    e.a.b(epi);
    e.a.bind(deopt);
    e.a.movz(0, 1);
    e.a.bind(epi);
    e.a.ldr(30, X_SP, nslots * 8);
    e.a.add_imm(X_SP, X_SP, frame);
    // Pop in the reverse order of the pushes above (x27/x28 went deepest),
    // otherwise every register comes back with a different register's value.
    e.a.ldp_post(X_K0 + 3, X_K0 + 4, POS16);
    e.a.ldp_post(X_K0 + 1, X_K0 + 2, POS16);
    e.a.ldp_post(X_HELP, X_K0, POS16);
    e.a.ldp_post(X_UPVALS, X_SMI, POS16);
    e.a.ldp_post(X_DEPTH, X_OUT, POS16);
    e.a.ret();

    if std::env::var_os("JHOL_JIT_DEBUG").is_some() {
        eprintln!("[jit] whole-function body: {} instructions", e.a.buf.len());
    }
    Some(e.a.finish())
}

/// Every slot a lowered instruction touches must lie inside the frame; the
/// compiler guarantees it, but native code writes memory, so it is checked.
fn regs_ok(r: &NReg, nslots: u32) -> bool {
    let ok = |s: u32| s < nslots;
    match *r {
        NReg::Copy { dst, src } => ok(dst) && ok(src),
        NReg::Bits { dst, .. } => ok(dst),
        NReg::Upval { dst, .. } => ok(dst),
        NReg::StoreUpval { src, .. } => ok(src),
        NReg::Arith { dst, l, r, .. } => ok(dst) && ok(l) && ok(r),
        NReg::ArithK { dst, l, .. } => ok(dst) && ok(l),
        NReg::Cmp { dst, l, r, .. } => ok(dst) && ok(l) && ok(r),
        NReg::CmpK { dst, l, .. } => ok(dst) && ok(l),
        NReg::Eq { dst, l, r, .. } => ok(dst) && ok(l) && ok(r),
        NReg::Branch { src, .. } => ok(src),
        NReg::Ret { src } => ok(src),
        NReg::Invoke {
            dst,
            callee,
            arg0,
            argc,
            ..
        } => ok(dst) && ok(callee) && (argc == 0 || ok(arg0 + argc - 1)),
        NReg::Jump { .. } | NReg::Nop => true,
    }
}

// Straight-line numeric functions need no native stack frame. Bind live VM
// slots to caller-saved registers, leaving boxed values only at the ABI edge.
// Calls/branches retain the existing spilling compiler and its deopt protocol.
fn emit_leaf(plan: &NPlan, nparams: u32, param_base: u32) -> Option<Vec<u32>> {
    let end = plan.ir.iter().position(|r| matches!(r, NReg::Ret { .. }))?;
    let body = &plan.ir[..=end];
    let mut slots = Vec::new();
    let mut need = |s: u32| {
        if !slots.contains(&s) {
            slots.push(s);
        }
    };
    for i in 0..nparams {
        need(param_base + i);
    }
    let mut after_store = false;
    for r in body {
        if after_store
            && !matches!(
                r,
                NReg::Nop | NReg::Copy { .. } | NReg::Upval { up: 0, .. } | NReg::Ret { .. }
            )
        {
            return None;
        }
        match *r {
            NReg::Nop => {}
            NReg::Copy { dst, src } => {
                need(dst);
                need(src);
            }
            NReg::Bits { dst, bits } => {
                if !bits_is_smi(bits) {
                    return None;
                }
                need(dst);
            }
            NReg::Upval { dst, up: 0 } => need(dst),
            NReg::StoreUpval { up: 0, src } => {
                need(src);
                after_store = true;
            }
            NReg::Arith { dst, l, r, .. } => {
                need(dst);
                need(l);
                need(r);
            }
            NReg::ArithK { dst, l, k, .. } => {
                if !bits_is_smi(plan.pool[k]) {
                    return None;
                }
                need(dst);
                need(l);
            }
            NReg::Ret { src } => need(src),
            _ => return None,
        }
    }
    if slots.len() > 7 {
        return None;
    }
    let reg = |s: u32| slots.iter().position(|&v| v == s).map(|i| 9 + i as u32);
    let mut a = Asm::new();
    let bail = a.label();
    a.cmp_imm(1, nparams);
    a.b_cond(3, bail);
    a.cmp_imm(2, NATIVE_MAX_FRAMES as u32);
    a.b_cond(2, bail);
    let mut defined = std::collections::HashSet::new();
    let mut upval_written = false;
    for i in 0..nparams {
        let slot = param_base + i;
        let r = reg(slot)?;
        a.ldr(r, 0, i * 8);
        a.ubfx(1, r, 44, 20);
        a.mov_const(2, SMI_TOP20);
        a.cmp(1, 2);
        a.b_cond(1, bail);
        a.sbfx44(r, r);
        defined.insert(slot);
    }
    let range = |a: &mut Asm, r: u32| {
        a.sbfx44(1, r);
        a.cmp(1, r);
        a.b_cond(1, bail);
    };
    for instruction in body {
        match *instruction {
            NReg::Nop => {}
            NReg::Copy { dst, src } => {
                if !defined.contains(&src) {
                    return None;
                }
                a.mov_reg(reg(dst)?, reg(src)?);
                defined.insert(dst);
            }
            NReg::Bits { dst, bits } => {
                a.mov_const(reg(dst)?, Value(bits).as_i64() as u64);
                defined.insert(dst);
            }
            NReg::Upval { dst, up: 0 } => {
                a.ldr(7, 4, 0);
                a.ldr(reg(dst)?, 7, CELL_VALUE_OFF);
                if !upval_written {
                    a.ubfx(1, reg(dst)?, 44, 20);
                    a.mov_const(2, SMI_TOP20);
                    a.cmp(1, 2);
                    a.b_cond(1, bail);
                }
                a.sbfx44(reg(dst)?, reg(dst)?);
                defined.insert(dst);
            }
            NReg::StoreUpval { up: 0, src } => {
                if !defined.contains(&src) {
                    return None;
                }
                a.mov_const(2, SMI_MASK);
                a.word(0x8a000000 | (2 << 16) | (reg(src)? << 5));
                a.mov_const(1, SMI_BOX_VAL);
                a.orr(0, 0, 1);
                a.ldr(7, 4, 0);
                a.str_(0, 7, CELL_VALUE_OFF);
                upval_written = true;
            }
            NReg::Arith { .. } | NReg::ArithK { .. } => {
                let (op, dst, l, operand, constant) = match *instruction {
                    NReg::Arith { op, dst, l, r } => (op, dst, l, r as usize, false),
                    NReg::ArithK { op, dst, l, k } => (op, dst, l, k, true),
                    _ => unreachable!(),
                };
                if !defined.contains(&l) {
                    return None;
                }
                let lhs = reg(l)?;
                let rhs = if !constant {
                    if !defined.contains(&(operand as u32)) {
                        return None;
                    }
                    reg(operand as u32)?
                } else {
                    a.mov_const(2, Value(plan.pool[operand]).as_i64() as u64);
                    2
                };
                match op {
                    0 => a.add_reg(0, lhs, rhs),
                    1 => a.sub_reg(0, lhs, rhs),
                    2 => {
                        a.mul_reg(0, lhs, rhs);
                        a.word(0x9b407c00 | (rhs << 16) | (lhs << 5) | 1);
                        a.word(0x937ffc00 | 5);
                        a.cmp(1, 5);
                        a.b_cond(1, bail);
                        let nz = a.label();
                        a.cbnz(0, nz);
                        a.eor_regs(1, lhs, rhs);
                        a.tbnz(1, 63, bail);
                        a.bind(nz);
                    }
                    3 => {
                        a.cbz(rhs, bail);
                        a.sdiv(1, lhs, rhs);
                        a.msub(0, 1, rhs, lhs);
                        let nz = a.label();
                        a.cbnz(0, nz);
                        a.tbnz(lhs, 63, bail);
                        a.bind(nz);
                    }
                    _ => return None,
                }
                range(&mut a, 0);
                a.mov_reg(reg(dst)?, 0);
                defined.insert(dst);
            }
            NReg::Ret { src } => {
                if !defined.contains(&src) {
                    return None;
                }
                a.mov_const(2, SMI_MASK);
                a.word(0x8a000000 | (2 << 16) | (reg(src)? << 5));
                a.mov_const(1, SMI_BOX_VAL);
                a.orr(0, 0, 1);
                a.str_(0, 3, 0);
                a.movz(0, 0);
                a.ret();
            }
            _ => return None,
        }
    }
    a.bind(bail);
    a.movz(0, 1);
    a.ret();
    Some(a.finish())
}

/// Numeric function that only calls itself. Values stay unboxed in registers;
/// the external entry boxes once. A changed closure binding deoptimizes.
fn emit_smi_recursion(plan: &NPlan, nparams: u32, param_base: u32) -> Option<Vec<u32>> {
    if nparams == 0 || nparams > 2 {
        return None;
    }
    let mut self_up = None;
    for r in &plan.ir {
        if let NReg::Upval { up, .. } = *r {
            if self_up.is_some() && self_up != Some(up) {
                return None;
            }
            self_up = Some(up);
        }
    }
    let self_up = self_up?;
    let mut numeric = Vec::new();
    for i in 0..nparams {
        numeric.push(param_base + i);
    }
    // A register holds the callee only until the next write. The same slot is
    // reused for a numeric result later in the function; treating it as the
    // callee for the whole body dropped `fib`.
    let mut holds: std::collections::HashSet<u32> = std::collections::HashSet::new();
    let need = |s: u32, numeric: &mut Vec<u32>| {
        if !numeric.contains(&s) {
            numeric.push(s);
        }
    };
    for r in &plan.ir {
        match *r {
            NReg::Nop | NReg::Jump { .. } | NReg::Upval { .. } => {}
            NReg::Copy { dst, src } => {
                if !holds.contains(&src) {
                    need(src, &mut numeric);
                    need(dst, &mut numeric);
                }
            }
            NReg::Bits { dst, bits } => {
                if !bits_is_smi(bits) {
                    return None;
                }
                need(dst, &mut numeric);
            }
            NReg::Arith { op, dst, l, r } => {
                if op > 1 {
                    return None;
                }
                need(dst, &mut numeric);
                need(l, &mut numeric);
                need(r, &mut numeric);
            }
            NReg::Cmp { dst, l, r, .. } => {
                need(dst, &mut numeric);
                need(l, &mut numeric);
                need(r, &mut numeric);
            }
            NReg::ArithK { op, dst, l, .. } => {
                if op > 1 {
                    return None;
                }
                need(dst, &mut numeric);
                need(l, &mut numeric);
            }
            NReg::CmpK { dst, l, .. } => {
                need(dst, &mut numeric);
                need(l, &mut numeric);
            }
            NReg::Branch { src, .. } => need(src, &mut numeric),
            NReg::Invoke {
                dst,
                callee,
                arg0,
                argc,
                ..
            } => {
                if !holds.contains(&callee) || argc != nparams {
                    return None;
                }
                need(dst, &mut numeric);
                for i in 0..argc {
                    need(arg0 + i, &mut numeric);
                }
            }
            NReg::Ret { src } => {
                if holds.contains(&src) {
                    return None;
                }
                need(src, &mut numeric);
            }
            _ => return None,
        }
        match *r {
            NReg::Upval { dst, .. } => {
                holds.insert(dst);
            }
            NReg::Copy { dst, src } => {
                if holds.contains(&src) {
                    holds.insert(dst);
                } else {
                    holds.remove(&dst);
                }
            }
            NReg::Bits { dst, .. }
            | NReg::Arith { dst, .. }
            | NReg::ArithK { dst, .. }
            | NReg::Cmp { dst, .. }
            | NReg::CmpK { dst, .. }
            | NReg::Invoke { dst, .. } => {
                holds.remove(&dst);
            }
            _ => {}
        }
    }
    if numeric.len() > 7 {
        return None;
    }
    if std::env::var_os("JHOL_JIT_DEBUG").is_some() {
        eprintln!(
            "smi numeric={numeric:?} self_up={self_up} pool={:?} ir={:?}",
            plan.pool, plan.ir
        );
    }
    let reg = |s: u32| numeric.iter().position(|&v| v == s).map(|i| 9 + i as u32);
    let slot_read = |r: &NReg, s: u32| match *r {
        NReg::Copy { src, .. } => src == s,
        NReg::Arith { l, r, .. } => l == s || r == s,
        NReg::ArithK { l, .. } | NReg::CmpK { l, .. } => l == s,
        NReg::Cmp { l, r, .. } => l == s || r == s,
        NReg::Branch { src, .. } | NReg::Ret { src } => src == s,
        NReg::Invoke {
            arg0, argc, callee, ..
        } => callee == s || (argc > 0 && (arg0..arg0 + argc).contains(&s)),
        _ => false,
    };
    let slot_write = |r: &NReg| match *r {
        NReg::Copy { dst, .. }
        | NReg::Bits { dst, .. }
        | NReg::Arith { dst, .. }
        | NReg::ArithK { dst, .. }
        | NReg::Cmp { dst, .. }
        | NReg::CmpK { dst, .. }
        | NReg::Invoke { dst, .. } => Some(dst),
        _ => None,
    };
    let mut a = Asm::new();
    let start = a.label();
    let raw = a.label();
    let ext_fail = a.label();
    let ext_leave = a.label();
    let raw_fail = a.label();
    let raw_depth = a.label();
    let raw_fit = a.label();
    let raw_child = a.label();
    let raw_status = a.label();
    let raw_ok = a.label();
    let raw_leave = a.label();
    let mut labels: Vec<usize> = (0..=plan.ir.len()).map(|_| a.label()).collect();
    let _ = &mut labels;

    a.bind(start);
    a.stp_pre(19, 20, NEG16);
    a.stp_pre(21, 24, NEG16);
    a.stp_pre(29, 30, NEG16);
    a.mov_reg(19, 2);
    a.mov_reg(20, 4);
    a.mov_reg(21, 3);
    a.mov_reg(24, 6);
    for i in 0..nparams {
        let r = reg(param_base + i)?;
        a.ldr(r, 0, i * 8);
        a.ubfx(1, r, 44, 20);
        a.mov_const(2, SMI_TOP20);
        a.cmp(1, 2);
        let ok_tag = a.label();
        a.b_cond(0, ok_tag);
        a.movz(0, 2);
        a.b(ext_leave);
        a.bind(ok_tag);
        a.sbfx44(r, r);
    }
    // Self-call only. The binding cannot change until we return to the
    // interpreter, so one check at the ABI edge covers every recursive entry.
    a.ldr(7, 20, self_up * 8);
    a.ldr(7, 7, CELL_VALUE_OFF);
    a.cmp(7, 24);
    let ok_up = a.label();
    a.b_cond(0, ok_up);
    a.movz(0, 3);
    a.b(ext_leave);
    a.bind(ok_up);
    a.bl(raw);
    let ok_child = a.label();
    a.cbz(1, ok_child);
    a.b(ext_leave);
    a.bind(ok_child);
    a.sbfx44(2, 0);
    a.cmp(2, 0);
    let ok_box = a.label();
    a.b_cond(0, ok_box);
    a.movz(0, 5);
    a.b(ext_leave);
    a.bind(ok_box);
    a.ubfx(0, 0, 0, 44);
    a.mov_const(2, SMI_BOX_VAL);
    a.orr(0, 0, 2);
    a.str_(0, 21, 0);
    a.movz(0, 0);
    a.b(ext_leave);
    a.bind(ext_fail);
    a.movz(0, 1);
    a.bind(ext_leave);
    a.ldp_post(29, 30, 2);
    a.ldp_post(21, 24, 2);
    a.ldp_post(19, 20, 2);
    a.ret();

    a.bind(raw);
    a.stp_pre(29, 30, NEG16);
    a.mov_const(2, NATIVE_MAX_FRAMES);
    a.cmp(19, 2);
    a.b_cond(2, raw_depth);

    let mut defined = std::collections::HashSet::new();
    let mut holds: std::collections::HashSet<u32> = std::collections::HashSet::new();
    for i in 0..nparams {
        defined.insert(param_base + i);
    }
    let fit = |a: &mut Asm, regn: u32| {
        a.sbfx44(1, regn);
        a.cmp(1, regn);
        a.b_cond(1, raw_fit);
    };
    for (ip, r) in plan.ir.iter().enumerate() {
        a.bind(labels[ip]);
        match *r {
            NReg::Nop => {}
            NReg::Upval { dst, .. } => {
                holds.insert(dst);
            }
            NReg::Copy { dst, src } => {
                if holds.contains(&src) {
                    holds.insert(dst);
                    continue;
                }
                holds.remove(&dst);
                if !defined.contains(&src) {
                    return None;
                }
                a.mov_reg(reg(dst)?, reg(src)?);
                defined.insert(dst);
            }
            NReg::Bits { dst, bits } => {
                a.mov_const(reg(dst)?, Value(bits).as_i64() as u64);
                holds.remove(&dst);
                defined.insert(dst);
            }
            NReg::Arith { op, dst, l, r } => {
                if !defined.contains(&l) || !defined.contains(&r) {
                    return None;
                }
                let (lr, rr) = (reg(l)?, reg(r)?);
                match op {
                    0 => a.add_reg(0, lr, rr),
                    1 => a.sub_reg(0, lr, rr),
                    _ => return None,
                }
                fit(&mut a, 0);
                a.mov_reg(reg(dst)?, 0);
                holds.remove(&dst);
                defined.insert(dst);
            }
            NReg::ArithK { op, dst, l, k } => {
                if !defined.contains(&l) {
                    return None;
                }
                a.mov_const(2, plan.pool[k]);
                match op {
                    0 => a.add_reg(0, reg(l)?, 2),
                    1 => a.sub_reg(0, reg(l)?, 2),
                    _ => return None,
                }
                fit(&mut a, 0);
                a.mov_reg(reg(dst)?, 0);
                holds.remove(&dst);
                defined.insert(dst);
            }
            NReg::Cmp { op, dst, l, r, .. } => {
                if !defined.contains(&l) || !defined.contains(&r) {
                    return None;
                }
                a.cmp(reg(l)?, reg(r)?);
                let cc = match op {
                    0 => 11u32,
                    1 => 12,
                    2 => 13,
                    _ => 10,
                };
                a.cset(reg(dst)?, cc);
                holds.remove(&dst);
                defined.insert(dst);
            }
            NReg::CmpK { op, dst, l, k, .. } => {
                if !defined.contains(&l) {
                    return None;
                }
                a.mov_const(2, plan.pool[k]);
                a.cmp(reg(l)?, 2);
                let cc = match op {
                    0 => 11u32,
                    1 => 12,
                    2 => 13,
                    _ => 10,
                };
                a.cset(reg(dst)?, cc);
                holds.remove(&dst);
                defined.insert(dst);
            }
            NReg::Branch { on_true, src, t, .. } => {
                if !defined.contains(&src) {
                    return None;
                }
                if on_true {
                    a.cbnz(reg(src)?, labels[t]);
                } else {
                    a.cbz(reg(src)?, labels[t]);
                }
            }
            NReg::Jump { t } => a.b(labels[t]),
            NReg::Invoke {
                dst,
                arg0,
                argc,
                ..
            } => {
                for i in 0..argc {
                    if !defined.contains(&(arg0 + i)) {
                        return None;
                    }
                    a.mov_reg(3 + i, reg(arg0 + i)?);
                }
                // Spill only values still read after this call. A branch below
                // the call makes the scan keep every register not yet overwritten.
                let mut live = 0u32;
                let mut killed = 0u32;
                for later in plan.ir.iter().skip(ip + 1) {
                    if matches!(later, NReg::Branch { .. } | NReg::Jump { .. }) {
                        for i in 0..numeric.len() {
                            let bit = 1u32 << i;
                            if killed & bit == 0 {
                                live |= bit;
                            }
                        }
                        break;
                    }
                    for (i, &slotv) in numeric.iter().enumerate() {
                        let bit = 1u32 << i;
                        if killed & bit != 0 {
                            continue;
                        }
                        if slot_read(later, slotv) {
                            live |= bit;
                        }
                        if slot_write(later) == Some(slotv) {
                            killed |= bit;
                        }
                    }
                }
                let mut spill: Vec<u32> = (0..numeric.len() as u32)
                    .filter(|i| live & (1 << i) != 0)
                    .map(|i| 9 + i)
                    .collect();
                if spill.len() % 2 == 1 {
                    spill.push(8);
                }
                if std::env::var_os("JHOL_JIT_DEBUG").is_some() {
                    eprintln!("smi spill {spill:?}");
                }
                for c in spill.chunks(2) {
                    a.stp_pre(c[0], c[1], NEG16);
                }
                for i in 0..nparams {
                    a.mov_reg(9 + i, 3 + i);
                }
                a.add_imm(19, 19, 1);
                a.bl(raw);
                a.sub_imm(19, 19, 1);
                a.mov_reg(3, 0);
                a.mov_reg(4, 1);
                for c in spill.chunks(2).rev() {
                    a.ldp_post(c[0], c[1], 2);
                }
                a.cbnz(4, raw_child);
                a.mov_reg(reg(dst)?, 3);
                holds.remove(&dst);
                defined.insert(dst);
            }
            NReg::Ret { src } => {
                if !defined.contains(&src) {
                    return None;
                }
                a.mov_reg(0, reg(src)?);
                a.b(raw_ok);
            }
            _ => return None,
        }
    }
    a.bind(labels[plan.ir.len()]);
    a.b(raw_fail);
    a.bind(raw_ok);
    a.movz(1, 0);
    a.b(raw_leave);
    a.bind(raw_depth);
    a.mov_reg(0, 19);
    a.b(raw_status);
    a.bind(raw_fit);
    a.movz(0, 6);
    a.b(raw_status);
    a.bind(raw_child);
    a.mov_reg(0, 3);
    a.b(raw_status);
    a.bind(raw_fail);
    a.movz(0, 7);
    a.bind(raw_status);
    a.movz(1, 1);
    a.bind(raw_leave);
    a.ldp_post(29, 30, 2);
    a.ret();
    Some(a.finish())
}

/// Compile a whole function, returning its native entry point. The `FnProto`
/// owns its immutable executable mapping until the last closure/frame drops.
/// `return p+q` or `return p*q` where p and q are the two parameters.
fn leaf_fold(plan: &NPlan, nparams: u32, param_base: u32) -> u8 {
    if nparams != 2 {
        return 0;
    }
    let Some(end) = plan.ir.iter().position(|r| matches!(r, NReg::Ret { .. })) else {
        return 0;
    };
    let mut origin: Vec<Option<u32>> = vec![None; 16];
    let mut mark = |origin: &mut Vec<Option<u32>>, slot: u32, from: u32| {
        let s = slot as usize;
        if s >= origin.len() {
            origin.resize(s + 1, None);
        }
        origin[s] = Some(from);
    };
    mark(&mut origin, param_base, param_base);
    mark(&mut origin, param_base + 1, param_base + 1);
    let mut fold = 0u8;
    let mut produced = u32::MAX;
    for r in &plan.ir[..=end] {
        match *r {
            NReg::Nop => {}
            NReg::Copy { dst, src } => {
                let Some(from) = origin.get(src as usize).copied().flatten() else {
                    return 0;
                };
                mark(&mut origin, dst, from);
            }
            NReg::Arith { op, dst, l, r } if op == 0 || op == 2 => {
                if fold != 0 {
                    return 0;
                }
                let ol = origin.get(l as usize).copied().flatten();
                let orr = origin.get(r as usize).copied().flatten();
                let p0 = param_base;
                let p1 = param_base + 1;
                if !matches!(
                    (ol, orr),
                    (Some(a), Some(b)) if (a == p0 && b == p1) || (a == p1 && b == p0)
                ) {
                    return 0;
                }
                fold = if op == 0 { 1 } else { 2 };
                produced = dst;
            }
            NReg::Ret { src } => {
                if src == produced && fold != 0 {
                    return fold;
                }
                return 0;
            }
            _ => return 0,
        }
    }
    0
}

pub fn try_compile_func(proto: &FnProto) -> Option<*const u8> {
    if !NATIVE_SUPPORTED
        || std::env::var_os("JHOL_NATIVE_OFF").is_some()
        || proto.jit_deopts.load(Ordering::Relaxed) >= NATIVE_DEOPT_LIMIT
    {
        return None;
    }
    if let Some(&(address, _)) = proto.jit_mapping.get() {
        return Some(address as *const u8);
    }
    let plan = lower(proto)?;
    if !plan.ir.iter().all(|r| regs_ok(r, proto.nslots as u32)) {
        return None;
    }
    let nslots = proto.nslots;
    let is_arrow = proto.is_arrow;
    let param_base = if is_arrow { 0u32 } else { 2u32 };
    let nparams = proto.nparams as u32;
    // Entry state: the guard proved every parameter is an Smi and lies inside
    // its slot. Everything else starts as `undefined`.
    let mut base = vec![0u8; nslots];
    for i in 0..nparams as usize {
        base[param_base as usize + i] = 1;
    }
    let kb = kind_of_slots(&plan, nslots, &base);
    let leaf = emit_leaf(&plan, nparams, param_base);
    let region_safe = leaf.is_some();
    let buf = leaf
        .or_else(|| emit_smi_recursion(&plan, nparams, param_base))
        .or_else(|| emit_func(&plan, &kb, nslots as u32, param_base, nparams))?;
    if !proto.code.iter().any(|i| i.op == Op::SetUpval) {
        proto.jit_pure.store(1, Ordering::Relaxed);
    }
    if region_safe {
        proto.jit_region_safe.store(1, Ordering::Relaxed);
        let fold = leaf_fold(&plan, nparams, param_base);
        if fold != 0 {
            proto.jit_fold.store(fold, Ordering::Relaxed);
        }
    }
    let byte_len = buf.len() * 4;
    let code_mem = alloc_exec(byte_len);
    if code_mem.is_null() {
        return None;
    }
    for (i, w) in buf.iter().enumerate() {
        unsafe { (code_mem as *mut u32).add(i).write(*w) };
    }
    if !seal_exec(code_mem, byte_len) {
        unsafe { release_exec(code_mem, byte_len) };
        return None;
    }
    if proto
        .jit_mapping
        .set((code_mem as usize, byte_len))
        .is_err()
    {
        unsafe { release_exec(code_mem, byte_len) };
        return proto
            .jit_mapping
            .get()
            .map(|&(address, _)| address as *const u8);
    }
    if std::env::var_os("JHOL_NATIVE_DUMP").is_some() {
        let bytes: Vec<u8> = buf.iter().flat_map(|w| w.to_le_bytes()).collect();
        let path = format!("/tmp/jp/native_{:x}.bin", proto as *const FnProto as usize);
        let _ = std::fs::write(&path, &bytes);
        eprintln!(
            "[dump] {} nslots={} nparams={} arrow={} nupvals={} words={} pool={:x?}",
            path,
            proto.nslots,
            proto.nparams,
            proto.is_arrow,
            proto.nupvals,
            buf.len(),
            plan.pool
        );
    }
    Some(code_mem)
}

/// Call another function from inside compiled code. Returns the callee's
/// status; `1` tells the compiled caller to deoptimize. Only a compiled,
/// upvalue-free callee may run natively, which is what keeps the whole native
/// call tree free of side effects.
///
/// # Safety
/// Called only from JIT-generated code, which passes `args` valid for `argc`
/// values and `out` valid for one write, with `depth` from its own counter. It
/// is never called from Rust (only its address is taken).
#[no_mangle]
pub unsafe extern "C" fn jhol_native_invoke(
    callee: u64,
    this: u64,
    args: *const Value,
    argc: u32,
    depth: u32,
    out: *mut Value,
) -> u32 {
    let v = Value::from_bits(callee);
    if !v.is_function() {
        return 1;
    }
    let f = unsafe { v.as_function() };
    let proto = match &f.kind {
        FnKind::Bytecode(p) => &**p,
        _ => return 1,
    };
    let entry = proto.jit_entry.load(Ordering::Relaxed);
    if entry.is_null() || proto.jit_pure.load(Ordering::Relaxed) == 0 {
        return 1;
    }
    let this = if proto.is_arrow { f.this_val.0 } else { this };
    unsafe { invoke_native_entry(entry, f.upvalues.as_ptr(), args, argc, depth, out, this, callee) }
}

#[inline]
unsafe fn invoke_native_entry(
    entry: *mut u8,
    upvalues: *const *mut crate::objects::JCell,
    args: *const Value,
    argc: u32,
    depth: u32,
    out: *mut Value,
    this: u64,
    callee: u64,
) -> u32 {
    let fun: NativeEntry = std::mem::transmute(entry);
    fun(args, argc, depth.wrapping_add(1), out, upvalues, this, callee)
}

// Boxed-register regions cover branches, cached own properties and pure native
// calls that the allocation-free numeric loop cannot lower. Each completed
// bytecode commits its result before the next guard. A failed guard returns the
// exact bytecode IP, so the interpreter never repeats an observable operation.
// Helpers neither allocate in the arena, collect, grow VM stacks nor call JS.
extern "C" fn region_binary(a: u64, b: u64, op: u32) -> u64 {
    use crate::value::ArithOp;
    let a = Value(a);
    let b = Value(b);
    if !a.is_number() || !b.is_number() {
        return Value::EMPTY.0;
    }
    let x = a.as_f64();
    let y = b.as_f64();
    let v = match op {
        0 => a.try_num_add(&b),
        1..=4 => a.try_num_binop(
            &b,
            match op {
                1 => ArithOp::Sub,
                2 => ArithOp::Mul,
                3 => ArithOp::Div,
                _ => ArithOp::Mod,
            },
        ),
        5 => Some(Value::from_bool(x < y)),
        6 => Some(Value::from_bool(x > y)),
        7 => Some(Value::from_bool(x <= y)),
        8 => Some(Value::from_bool(x >= y)),
        9 => Some(Value::from_bool(x == y)),
        10..=15 => {
            let x = if a.is_smi() {
                a.as_i64() as i32
            } else {
                crate::vm::js_to_int32(x)
            };
            let y = if b.is_smi() {
                b.as_i64() as i32
            } else {
                crate::vm::js_to_int32(y)
            };
            Some(crate::vm::int32_bitop((op - 10) as u8, x, y))
        }
        _ => None,
    };
    v.unwrap_or(Value::EMPTY).0
}

extern "C" fn region_truthy(bits: u64) -> u64 {
    Value(bits).to_bool() as u64
}

extern "C" fn region_property(bits: u64, cache: *const Ic) -> u64 {
    let value = Value(bits);
    let ic = unsafe { &*cache };
    // Numbers and booleans own nothing and share `Number.prototype` — mirror
    // the interpreter's num IC here so a JIT'd loop doing `(i % 10).toString`
    // resolves the method with a shape check and one slot read instead of a
    // call into the interpreter per iteration.
    if value.is_smi() || value.is_double() || value.is_bool() {
        if ic.has_num {
            let vm = region_vm();
            if !vm.is_null() {
                let p = unsafe { (*vm).number_proto() };
                if p.is_object() {
                    let o = unsafe { p.as_object() };
                    if o.shape == ic.num_shape {
                        if let Some((k, v)) = o.get_at(ic.num_slot as usize) {
                            if k == ic.key {
                                return v.0;
                            }
                        }
                    }
                }
            }
        }
        return region_vm_prop(value, ic.key, cache);
    }
    if value.is_string() {
        if let crate::objects::PropKey::Sym(sym) = ic.key {
            if crate::arena::active().sym_name(sym) == "length" {
                return Value::from_i64(unsafe { value.as_string() }.utf16_len as i64).0;
            }
        }
        // String-receiver methods (`s.charCodeAt` in a hash loop) mirror the
        // interpreter's string IC: one shape check, one slot read.
        if ic.has_str {
            let vm = region_vm();
            if !vm.is_null() {
                let p = unsafe { (*vm).string_proto() };
                if p.is_object() {
                    let o = unsafe { p.as_object() };
                    if o.shape == ic.str_shape {
                        if let Some((k, v)) = o.get_at(ic.str_slot as usize) {
                            if k == ic.key {
                                return v.0;
                            }
                        }
                    }
                }
            }
        }
        return region_vm_prop(value, ic.key, cache);
    }
    if value.is_array() {
        if let crate::objects::PropKey::Sym(sym) = ic.key {
            if crate::arena::active().sym_name(sym) == "length" {
                return Value::from_i64(unsafe { value.as_array() }.len as i64).0;
            }
        }
        // Method lookups on arrays (`objs.push` in a build loop) mirror the
        // interpreter's array IC: validate Array.prototype's shape, read one
        // slot — no interpreter call per iteration.
        if ic.has_arr {
            let vm = region_vm();
            if !vm.is_null() {
                let p = unsafe { (*vm).array_proto() };
                if p.is_object() {
                    let o = unsafe { p.as_object() };
                    if o.shape == ic.arr_shape {
                        if let Some((k, v)) = o.get_at(ic.arr_slot as usize) {
                            if k == ic.key {
                                return v.0;
                            }
                        }
                    }
                }
            }
        }
        return region_vm_prop(value, ic.key, cache);
    }
    if !value.is_object() {
        return region_vm_prop(value, ic.key, cache);
    }
    let obj = unsafe { value.as_object() };
    if obj.hdr.flags & crate::objects::FLAG_ACCESSOR != 0 {
        return Value::EMPTY.0;
    }
    ic.slot_for(obj.shape)
        .and_then(|slot| obj.get_at(slot as usize))
        .filter(|(key, _)| *key == ic.key)
        .map_or(Value::EMPTY.0, |(_, v)| v.0)
}

extern "C" fn region_index(bits: u64, index: u64) -> u64 {
    let obj = Value(bits);
    let index = Value(index);
    if !obj.is_array() || !index.is_smi() {
        return Value::EMPTY.0;
    }
    let i = index.as_i64();
    if i < 0 || i > u32::MAX as i64 {
        return Value::EMPTY.0;
    }
    unsafe { obj.as_array() }
        .get_raw(i as u32)
        .unwrap_or(Value::EMPTY)
        .0
}

// Private to one native invocation. Reset entry pointers at every region entry:
// these pointers never survive a safepoint, collection, or function replacement.
#[repr(C)]
struct RegionCallTarget {
    callee: u64,
    entry: *mut u8,
    upvalues: *const *mut JCell,
    lexical_this: u64,
    is_arrow: u64,
    /// Non-zero: audited native leaf. `entry` is then a non-null sentinel, not code.
    native: u64,
    /// 1 add, 2 mul. Two Smi parameters, no other effect.
    fold: u64,
}

extern "C" fn region_prepare_call(bits: u64, target: *mut RegionCallTarget) -> u32 {
    let value = Value(bits);
    if !value.is_function() {
        return 0;
    }
    let f = unsafe { value.as_function() };
    if let crate::objects::FnKind::Native(nf) = f.kind {
        let flags = f.hdr.flags;
        if flags
            & (crate::objects::FLAG_NATIVE_LEAF | crate::objects::FLAG_NATIVE_PRIMITIVE_LEAF)
            == 0
        {
            return 0;
        }
        unsafe {
            target.write(RegionCallTarget {
                callee: bits,
                entry: 1 as *mut u8,
                upvalues: std::ptr::null(),
                lexical_this: 0,
                is_arrow: 0,
                native: nf as usize as u64,
                fold: 0,
            });
        }
        return 1;
    }
    let crate::objects::FnKind::Bytecode(ref proto) = f.kind else {
        return 0;
    };
    if proto.jit_entry.load(Ordering::Relaxed).is_null() {
        if let Some(entry) = try_compile_func(proto) {
            proto.jit_entry.store(entry as *mut u8, Ordering::Relaxed);
        }
    }
    let entry = proto.jit_entry.load(Ordering::Relaxed);
    // A committing leaf can fail only before its writes and succeeds with an
    // Smi result, so the region cannot repeat its call on a result-type miss.
    // Later instruction guards resume after the already-completed call.
    if entry.is_null()
        || (proto.jit_pure.load(Ordering::Relaxed) == 0
            && proto.jit_region_safe.load(Ordering::Relaxed) == 0)
    {
        return 0;
    }
    unsafe {
        target.write(RegionCallTarget {
            callee: bits,
            entry,
            upvalues: f.upvalues.as_ptr(),
            lexical_this: f.this_val.0,
            is_arrow: proto.is_arrow as u64,
            native: 0,
            fold: proto.jit_fold.load(Ordering::Relaxed) as u64,
        });
    }
    1
}

thread_local! {
    static REGION_VM: std::cell::Cell<*mut crate::vm::Vm> =
        const { std::cell::Cell::new(std::ptr::null_mut()) };
    static REGION_CONSTS: std::cell::Cell<*const crate::bytecode::ConstVal> =
        const { std::cell::Cell::new(std::ptr::null()) };
}

pub(crate) fn bind_region_vm(vm: *mut crate::vm::Vm) {
    REGION_VM.with(|c| c.set(vm));
}

pub(crate) fn bind_region_consts(consts: *const crate::bytecode::ConstVal) {
    REGION_CONSTS.with(|c| c.set(consts));
}

fn region_vm() -> *mut crate::vm::Vm {
    REGION_VM.with(|c| c.get())
}

fn region_vm_prop(value: Value, key: crate::objects::PropKey, ic: *const Ic) -> u64 {
    let vm = region_vm();
    if vm.is_null() {
        return Value::EMPTY.0;
    }
    let slot = if ic.is_null() {
        None
    } else {
        Some(unsafe { &mut *(ic as *mut Ic) })
    };
    match unsafe { (*vm).get_prop_ic(value, key, slot) } {
        Ok(v) => v.0,
        Err(_) => Value::EMPTY.0,
    }
}

/// `x6` is the native fn, `x7` the callee value. Returns 0 on success.
extern "C" fn region_enter_native(
    args: *const Value,
    argc: u32,
    _depth: u32,
    out: *mut Value,
    _up: *const *mut JCell,
    this: u64,
    native: u64,
    callee: u64,
) -> u32 {
    let vm = region_vm();
    if vm.is_null() || native == 0 || out.is_null() {
        return 1;
    }
    if native == crate::builtins::n_push as usize as u64 && argc == 1 {
        let this_v = Value(this);
        if this_v.is_array() {
            let a = unsafe { this_v.as_array_mut() };
            a.push(unsafe { *args });
            unsafe { *out = Value::from_i32(a.len as i32) };
            return 0;
        }
    }
    // Digit `toString` and ascii `charCodeAt` are the string-build loop. The
    // generic leaf call checks flags and builds a slice; these two results are
    // a cached atom or one byte load.
    if native == crate::builtins::n_char_code_at as usize as u64 && argc == 1 {
        let this_v = Value(this);
        let arg0 = unsafe { *args };
        if this_v.is_string() && arg0.is_smi() {
            let i = arg0.as_i64();
            let unit = if i < 0 {
                None
            } else {
                unsafe { this_v.as_string() }.code_unit(i as usize)
            };
            unsafe {
                *out = unit.map_or(Value::NAN, |u| Value::from_i32(u as i32));
            }
            return 0;
        }
    }
    if native == crate::builtins::n_number_to_string as usize as u64 && argc == 0 {
        let this_v = Value(this);
        if this_v.is_smi() {
            let d = this_v.as_i64();
            if (0..10).contains(&d) {
                let cached = unsafe { &*vm }.digit_atoms[d as usize];
                if !cached.is_undefined() {
                    unsafe { *out = cached };
                    return 0;
                }
            }
        }
    }
    let callee_v = Value(callee);
    if !callee_v.is_function() {
        return 1;
    }
    let flags = unsafe { callee_v.as_function() }.hdr.flags;
    let this_v = Value(this);
    let n = argc as usize;
    if n > 8 {
        return 1;
    }
    let primitive = |v: Value| !v.is_ptr() || v.is_string();
    let arg_slice = unsafe { std::slice::from_raw_parts(args, n) };
    if flags & crate::objects::FLAG_NATIVE_LEAF == 0
        && (flags & crate::objects::FLAG_NATIVE_PRIMITIVE_LEAF == 0
            || !primitive(this_v)
            || !arg_slice.iter().copied().all(primitive))
    {
        return 1;
    }
    let fun: crate::objects::NativeFn = unsafe { std::mem::transmute(native as usize) };
    match fun(unsafe { &mut *vm }, this_v, arg_slice) {
        Ok(v) => {
            unsafe { *out = v };
            0
        }
        Err(_) => 1,
    }
}

extern "C" fn region_append(slot: u32, rhs: u64) -> u32 {
    let vm = region_vm();
    if vm.is_null() {
        return 1;
    }
    unsafe { &mut *vm }.jit_append_str(slot, Value(rhs))
}

extern "C" fn region_new_lit(
    regs: *mut Value,
    dest: u32,
    val_base: u32,
    ic: *mut Ic,
    consts: *const crate::bytecode::ConstVal,
    key_idx: u32,
) -> u32 {
    let vm = region_vm();
    if vm.is_null() {
        return 1;
    }
    unsafe { &mut *vm }.jit_new_object_lit(regs, dest, val_base, ic, consts, key_idx)
}

extern "C" fn region_set_prop(obj: u64, val: u64, ic: *mut Ic) -> u32 {
    let vm = region_vm();
    if vm.is_null() {
        return 1;
    }
    unsafe { &mut *vm }.jit_set_prop(Value(obj), Value(val), ic)
}

extern "C" fn region_invoke(
    regs: *mut Value,
    callee: u32,
    args: u32,
    argc: u32,
    out: u32,
    this_slot: u32,
    depth: u32,
) -> u32 {
    let mut target = std::mem::MaybeUninit::<RegionCallTarget>::uninit();
    if region_prepare_call(
        unsafe { (*regs.add(callee as usize)).0 },
        target.as_mut_ptr(),
    ) == 0
    {
        return 0;
    }
    let target = unsafe { target.assume_init() };
    let this = if target.is_arrow != 0 {
        target.lexical_this
    } else if this_slot == u32::MAX {
        Value::UNDEFINED.0
    } else {
        unsafe { (*regs.add(this_slot as usize)).0 }
    };
    if target.native != 0 {
        return unsafe {
            (region_enter_native(
                regs.add(args as usize),
                argc,
                depth,
                regs.add(out as usize),
                target.upvalues,
                this,
                target.native,
                target.callee,
            ) == 0) as u32
        };
    }
    // Whole-function entries return zero on success; regions expect a
    // nonzero success flag. Preserve that boundary rather than deoptimizing
    // every successful call and executing it again in the interpreter.
    unsafe {
        (invoke_native_entry(
            target.entry,
            target.upvalues,
            regs.add(args as usize),
            argc,
            depth,
            regs.add(out as usize),
            this,
            target.callee,
        ) == 0) as u32
    }
}

/// A region has no allocation or callback safepoints: all heap values remain
/// boxed on the VM operand stack; verified native leaves use the no-GC ABI.
/// # Safety
/// `ics` must point to the inline-cache array of the function `ins` belongs to,
/// valid for every index the region reads (`ip < ins.len()`), for the duration
/// of the call; `arena_base` must be the live arena base address.
pub unsafe fn try_compile_region(
    ins: &[Instr],
    top: usize,
    nslots: u32,
    consts: &mut dyn FnMut(usize) -> Option<f64>,
    ics: *const Ic,
    arena_base: usize,
) -> Option<JitLoop> {
    if !NATIVE_SUPPORTED || std::env::var("JHOL_JIT_ENABLE").as_deref() == Ok("0") {
        return None;
    }
    let header = *ins.get(top)?;
    let fused = header.op == Op::JumpLocalCmp;
    let test = header.a;
    let cmp = header.b >> 24;
    let exit = if fused {
        if test >= nslots || cmp > 3 {
            return None;
        }
        header.c as usize
    } else {
        // Keep the entire test in the region: a local bound or array length
        // must be read on each iteration. Require an actual closing back-edge
        // so an early body branch cannot be mistaken for the loop's exit.
        let exit = ins
            .iter()
            .skip(top)
            .take(8)
            .find(|x| x.op == Op::JumpIfFalse)?
            .c as usize;
        let last = ins.get(exit.checked_sub(1)?)?;
        if !matches!(last.op, Op::Jump | Op::LocalAddKJ) || last.c as usize != top {
            return None;
        }
        exit
    };
    if nslots > 4096 || exit <= top || exit > ins.len() || exit - top > 192 {
        return None;
    }
    let bound = if fused {
        Value::from_f64(consts((header.b & 0x00ff_ffff) as usize)?).0
    } else {
        0
    };
    let body_start = top + usize::from(fused);
    let slot = |n: u32| n < nslots;
    let binary_op = |op: Op| match op {
        Op::Add => Some(0),
        Op::Sub => Some(1),
        Op::Mul => Some(2),
        Op::Div => Some(3),
        Op::Mod => Some(4),
        Op::Lt => Some(5),
        Op::Gt => Some(6),
        Op::Le => Some(7),
        Op::Ge => Some(8),
        Op::Eq | Op::StrictEq => Some(9),
        Op::BitAnd => Some(10),
        Op::BitOr => Some(11),
        Op::BitXor => Some(12),
        Op::Shl => Some(13),
        Op::Shr => Some(14),
        Op::UShr => Some(15),
        _ => None,
    };
    // Refuse the entire region before emitting code for any observable slow op.
    for x in &ins[body_start..exit] {
        let valid = match x.op {
            Op::Move | Op::GetLocal | Op::SetLocal => slot(x.a) && slot(x.b),
            Op::LoadK => slot(x.a) && consts(x.b as usize).is_some(),
            Op::LoadTrue | Op::LoadFalse | Op::LoadNull | Op::LoadUndefined => slot(x.a),
            Op::LocalAdd => slot(x.a) && slot(x.b),
            Op::LocalAddK | Op::LocalAddKJ => {
                slot(x.a)
                    && consts(x.b as usize).is_some()
                    && (x.op != Op::LocalAddKJ || x.c as usize == top)
            }
            Op::BinOpK(sub) => sub <= 8 && slot(x.a) && slot(x.b) && consts(x.c as usize).is_some(),
            Op::GetProp => slot(x.a) && slot(x.b),
            Op::GetIndex => slot(x.a) && slot(x.b) && slot(x.c),
            Op::SetProp => slot(x.a) && slot(x.c),
            Op::AppendStr => slot(x.a) && slot(x.b),
            Op::NewObjectLit => {
                slot(x.a)
                    && !REGION_CONSTS.with(|c| c.get().is_null())
                    && matches!(
                        unsafe { &*REGION_CONSTS.with(|c| c.get()).add(x.b as usize) },
                        crate::bytecode::ConstVal::Keys(_)
                    )
            }
            Op::Call | Op::CallThis => {
                slot(x.a)
                    && slot(x.b)
                    && x.b
                        .checked_add(x.c)
                        .and_then(|n| n.checked_add(if x.op == Op::CallThis { 2 } else { 1 }))
                        .is_some_and(|end| end <= nslots)
            }
            Op::Jump => x.c as usize == top || (x.c as usize > top && x.c as usize <= exit),
            Op::JumpIfTrue | Op::JumpIfFalse => {
                slot(x.a) && x.c as usize > top && x.c as usize <= exit
            }
            op if binary_op(op).is_some() => slot(x.a) && slot(x.b) && slot(x.c),
            _ => false,
        };
        if !valid {
            return None;
        }
    }
    let mut call_sites: Vec<_> = (body_start..exit)
        .filter(|&ip| matches!(ins[ip].op, Op::Call | Op::CallThis))
        .collect();
    // Bound the native stack frame and its immediate offsets. Larger regions
    // retain the existing verified call bridge.
    if call_sites.len() > 32 {
        call_sites.clear();
    }
    let target_size = std::mem::size_of::<RegionCallTarget>() as u32;
    let cache_bytes = (call_sites.len() as u32 * target_size + 15) & !15;
    let mut a = Asm::new();
    let labels: Vec<_> = (top..=exit).map(|_| a.label()).collect();
    let bailouts: Vec<_> = (top..exit).map(|_| a.label()).collect();
    // Retain loop-carried integers in callee-saved registers. Boxed stack
    // slots remain authoritative at every instruction boundary and across calls.
    let mut carried = if fused { vec![test] } else { Vec::new() };
    for x in &ins[body_start..exit] {
        if matches!(x.op, Op::LocalAdd | Op::LocalAddK | Op::LocalAddKJ)
            && !carried.contains(&x.a)
            && carried.len() < 3
        {
            carried.push(x.a);
        }
    }
    let cached = |slot: u32| {
        carried
            .iter()
            .position(|&s| s == slot)
            .map(|n| 26 + n as u32)
    };
    let entry_bail = a.label();
    a.stp_pre(19, 20, 0x7e);
    a.stp_pre(21, 22, 0x7e);
    a.stp_pre(23, 24, 0x7e);
    a.stp_pre(25, 26, 0x7e);
    a.stp_pre(27, 28, 0x7e);
    a.stp_pre(29, 30, 0x7e);
    a.mov_reg(19, 0);
    a.mov_reg(20, 1);
    a.mov_const(21, SMI_MASK);
    a.mov_const(22, SMI_BOX_VAL);
    a.mov_const(23, SMI_TOP20);
    if cache_bytes != 0 {
        a.sub_imm(31, 31, cache_bytes);
        a.add_imm(24, 31, 0);
        a.movz(0, 0);
        for i in 0..call_sites.len() as u32 {
            a.str_(
                0,
                24,
                i * target_size + std::mem::offset_of!(RegionCallTarget, entry) as u32,
            );
            a.str_(
                0,
                24,
                i * target_size + std::mem::offset_of!(RegionCallTarget, native) as u32,
            );
        }
    }
    // Field snapshots for a literal that never escapes the block. x25 is
    // callee-saved, so a later call keeps the pointer.
    a.sub_imm(31, 31, 64);
    a.add_imm(25, 31, 0);
    for &slot in &carried {
        let r = cached(slot)?;
        a.ldr(r, 19, slot * 8);
        a.ubfx(2, r, 44, 20);
        a.cmp(2, 23);
        a.b_cond(1, entry_bail);
        a.sbfx44(r, r);
    }
    let numeric = |a: &mut Asm, slot: u32, r: u32, bail| {
        if let Some(src) = cached(slot) {
            a.mov_reg(r, src);
        } else {
            a.ldr(r, 19, slot * 8);
            a.ubfx(2, r, 44, 20);
            a.cmp(2, 23);
            a.b_cond(1, bail);
            a.sbfx44(r, r);
        }
    };
    let save = |a: &mut Asm, dst: u32, bail| {
        if let Some(r) = cached(dst) {
            a.ubfx(2, 0, 44, 20);
            a.cmp(2, 23);
            a.b_cond(1, bail);
            a.sbfx44(r, 0);
        }
        a.str_(0, 19, dst * 8);
    };
    let call = |a: &mut Asm, address: usize| {
        a.mov_const(16, address as u64);
        a.blr(16);
    };
    let guard = |a: &mut Asm, bailout| {
        a.mov_const(9, Value::EMPTY.0);
        a.cmp(0, 9);
        a.b_cond(0, bailout);
    };
    // A literal that is only read and written in the same straight-line block
    // never becomes a heap object. The field values already sit in registers.
    let mut virt_get: Vec<Option<u32>> = vec![None; ins.len()];
    let mut virt_skip = vec![false; ins.len()];
    let mut virt_copy: Vec<Vec<(u32, u32)>> = vec![Vec::new(); ins.len()];
    {
        let pool = REGION_CONSTS.with(|c| c.get());
        let escapes = |y: Instr, dest: u32| -> bool {
            match y.op {
                Op::Call => {
                    let end = y.b.saturating_add(y.c).saturating_add(1);
                    dest >= y.b && dest < end
                }
                Op::CallThis => {
                    let end = y.b.saturating_add(y.c).saturating_add(2);
                    dest >= y.b && dest < end
                }
                Op::Jump => false,
                Op::JumpIfTrue | Op::JumpIfFalse => y.a == dest,
                _ => y.a == dest || y.b == dest || y.c == dest,
            }
        };
        let mut ip = body_start;
        while ip < exit {
            if ins[ip].op != Op::NewObjectLit {
                ip += 1;
                continue;
            }
            let lit = ins[ip];
            let keys: Vec<crate::objects::PropKey> =
                match unsafe { &*pool.add(lit.b as usize) } {
                    crate::bytecode::ConstVal::Keys(k) if k.len() <= 8 => k.to_vec(),
                    _ => {
                        ip += 1;
                        continue;
                    }
                };
            let root = lit.a;
            let mut aliases = vec![root];
            let mut fields: Vec<(crate::objects::PropKey, u32, usize)> = keys
                .into_iter()
                .enumerate()
                .map(|(i, k)| (k, lit.c + i as u32, ip))
                .collect();
            let mut gets: Vec<(usize, u32)> = Vec::new();
            let mut ok = true;
            let mut why = "";
            let mut j = ip + 1;
            while j < exit {
                let y = ins[j];
                if y.op == Op::NewObjectLit && y.a == root {
                    break;
                }
                if matches!(y.op, Op::Jump | Op::JumpIfTrue | Op::JumpIfFalse) {
                    let t = y.c as usize;
                    if (body_start..exit).contains(&t) {
                        let mut k = t;
                        while k < exit {
                            let z = ins[k];
                            if z.op == Op::NewObjectLit && z.a == root {
                                break;
                            }
                            if escapes(z, root) {
                                ok = false;
                                why = "join";
                                break;
                            }
                            k += 1;
                        }
                    }
                    break;
                }
                if y.op == Op::GetProp && aliases.contains(&y.b) {
                    let ic = unsafe { &*ics.add(j) };
                    let found = fields.iter().position(|(k, _, _)| *k == ic.key);
                    if ic.state == 0 || found.is_none() || aliases.contains(&y.a) {
                        ok = false;
                        why = if ic.state == 0 { "get-state" } else if found.is_none() { "get-key" } else { "get-root" };
                        break;
                    }
                    aliases.retain(|s| *s != y.a);
                    gets.push((j, found.unwrap() as u32));
                } else if y.op == Op::SetProp && aliases.contains(&y.a) {
                    let ic = unsafe { &*ics.add(j) };
                    if aliases.contains(&y.c) {
                        ok = false;
                        why = "set-value";
                        break;
                    }
                    if let Some(f) = fields.iter_mut().find(|(k, _, _)| *k == ic.key) {
                        f.1 = y.c;
                        f.2 = j;
                    } else {
                        fields.push((ic.key, y.c, j));
                    }
                } else if matches!(y.op, Op::Move | Op::GetLocal | Op::SetLocal)
                    && aliases.contains(&y.b)
                    && y.a != root
                {
                    if !aliases.contains(&y.a) {
                        aliases.push(y.a);
                    }
                } else if aliases.contains(&y.a) {
                    aliases.retain(|s| *s != y.a);
                    if aliases.is_empty() {
                        ok = false;
                        why = "dead";
                        break;
                    }
                } else if aliases.iter().any(|s| escapes(y, *s)) {
                    ok = false;
                    why = "escape";
                    if std::env::var_os("JHOL_JIT_DEBUG").is_some() {
                        eprintln!(
                            "[jit] virt escape ip={j} op={:?} a={} b={} c={} aliases={:?}",
                            y.op, y.a, y.b, y.c, aliases
                        );
                    }
                    break;
                }
                j += 1;
            }
            if std::env::var_os("JHOL_JIT_DEBUG").is_some() {
                eprintln!("[jit] virt ip={ip} {} {why}", if ok { "ok" } else { "no" });
            }
            if ok {
                virt_skip[ip] = true;
                virt_copy[ip] = fields
                    .iter()
                    .enumerate()
                    .filter(|(_, f)| f.2 == ip)
                    .map(|(i, f)| (f.1, i as u32))
                    .collect();
                for (i, f) in fields.iter().enumerate() {
                    if f.2 != ip {
                        virt_skip[f.2] = true;
                        virt_copy[f.2].push((f.1, i as u32));
                    }
                }
                for (g, slot) in gets {
                    virt_get[g] = Some(slot);
                }
            }
            ip += 1;
        }
    }
    for ip in top..exit {
        a.bind(labels[ip - top]);
        let x = ins[ip];
        let bailout = bailouts[ip - top];
        if fused && ip == top {
            if Value(bound).is_smi() {
                numeric(&mut a, test, 0, bailout);
                a.mov_const(1, Value(bound).as_i64() as u64);
                a.cmp(0, 1);
                a.b_cond(
                    match cmp {
                        0 => 10,
                        1 => 13,
                        2 => 12,
                        _ => 11,
                    },
                    labels[exit - top],
                );
            } else {
                a.ldr(0, 19, test * 8);
                a.mov_const(1, bound);
                a.movz(2, (5 + cmp) as u64);
                call(&mut a, region_binary as *const () as usize);
                guard(&mut a, bailout);
                a.mov_const(9, Value::TRUE.0);
                a.cmp(0, 9);
                a.b_cond(1, labels[exit - top]);
            }
            continue;
        }
        let bin = if let Some(op) = binary_op(x.op) {
            Some((x.a, x.b, Some(x.c), None, op))
        } else {
            match x.op {
                Op::LocalAdd => Some((x.a, x.a, Some(x.b), None, 0)),
                Op::LocalAddK | Op::LocalAddKJ => Some((x.a, x.a, None, Some(x.b), 0)),
                Op::BinOpK(sub) => Some((x.a, x.b, None, Some(x.c), sub as u32)),
                _ => None,
            }
        };
        if let Some((dst, l, r, k, op)) = bin {
            a.ldr(0, 19, l * 8);
            if let Some(r) = r {
                a.ldr(1, 19, r * 8);
            } else {
                a.mov_const(1, Value::from_f64(consts(k? as usize)?).0);
            }
            // Mul/mod that leave Smi stay in registers. Overflow or a non-Smi
            // operand calls the same helper and continues; bailing here used
            // to disable the whole loop after one wide product.
            if op == 2 || op == 4 {
                let slow = a.label();
                let done = a.label();
                let smi_or_slow = |a: &mut Asm, regn: u32| {
                    a.ubfx(2, regn, 44, 20);
                    a.cmp(2, 23);
                    a.b_cond(1, slow);
                    a.sbfx44(regn, regn);
                };
                smi_or_slow(&mut a, 0);
                if r.is_some() {
                    smi_or_slow(&mut a, 1);
                } else {
                    a.ubfx(2, 1, 44, 20);
                    a.cmp(2, 23);
                    a.b_cond(1, bailout);
                    a.sbfx44(1, 1);
                }
                if op == 2 {
                    a.word(0x9b407c02 | (1 << 16));
                    a.eor_regs(5, 0, 1);
                    a.mul_reg(0, 0, 1);
                    a.word(0x937ffc06);
                    a.cmp(2, 6);
                    a.b_cond(1, slow);
                    let nz = a.label();
                    a.cbnz(0, nz);
                    a.tbnz(5, 63, slow);
                    a.bind(nz);
                } else {
                    a.cbz(1, slow);
                    a.mov_reg(5, 0);
                    a.sdiv(2, 0, 1);
                    a.msub(0, 2, 1, 0);
                    let nz = a.label();
                    a.cbnz(0, nz);
                    a.tbnz(5, 63, slow);
                    a.bind(nz);
                }
                a.sbfx44(2, 0);
                a.cmp(2, 0);
                a.b_cond(1, slow);
                a.word(0x8a150000);
                a.orr(0, 0, 22);
                a.b(done);
                a.bind(slow);
                a.ldr(0, 19, l * 8);
                if let Some(rs) = r {
                    a.ldr(1, 19, rs * 8);
                } else {
                    a.mov_const(1, Value::from_f64(consts(k? as usize)?).0);
                }
                a.movz(2, op as u64);
                call(&mut a, region_binary as *const () as usize);
                guard(&mut a, bailout);
                a.bind(done);
                save(&mut a, dst, bailout);
                if x.op == Op::LocalAddKJ {
                    a.b(labels[0]);
                }
                continue;
            }
            // Numeric Smi operations stay in machine registers. A guard exits
            // before this instruction writes its destination; non-integers use
            // the interpreter's full JavaScript conversion semantics.
            if matches!(op, 0..=2 | 4..=12) {
                numeric(&mut a, l, 0, bailout);
                if let Some(r) = r {
                    numeric(&mut a, r, 1, bailout);
                } else {
                    a.ubfx(2, 1, 44, 20);
                    a.cmp(2, 23);
                    a.b_cond(1, bailout);
                    a.sbfx44(1, 1);
                }
                match op {
                    0 => a.add_reg(0, 0, 1),
                    1 => a.sub_reg(0, 0, 1),
                    2 => {
                        a.word(0x9b407c02 | (1 << 16)); // smulh x2,x0,x1
                        a.eor_regs(5, 0, 1);
                        a.mul_reg(0, 0, 1);
                        a.word(0x937ffc06); // asr x6,x0,63
                        a.cmp(2, 6);
                        a.b_cond(1, bailout);
                        let nz = a.label();
                        a.cbnz(0, nz);
                        a.tbnz(5, 63, bailout);
                        a.bind(nz);
                    }
                    4 => {
                        a.cbz(1, bailout);
                        a.mov_reg(5, 0);
                        a.sdiv(2, 0, 1);
                        a.msub(0, 2, 1, 0);
                        let nz = a.label();
                        a.cbnz(0, nz);
                        a.tbnz(5, 63, bailout);
                        a.bind(nz);
                    }
                    5..=9 => {
                        a.cmp(0, 1);
                        let cond = match op {
                            5 => 11,
                            6 => 12,
                            7 => 13,
                            8 => 10,
                            _ => 0,
                        };
                        a.word(0x9a9f07e0 | ((cond ^ 1) << 12)); // cset x0,cond
                        a.mov_const(1, Value::FALSE.0);
                        a.eor_regs(0, 0, 1);
                    }
                    10..=12 => {
                        a.word(match op {
                            10 => 0x0a010000,
                            11 => 0x2a010000,
                            _ => 0x4a010000,
                        });
                        a.word(0x93407c00); // sxtw x0,w0
                    }
                    _ => unreachable!(),
                }
                if !(5..=9).contains(&op) {
                    a.sbfx44(2, 0);
                    a.cmp(2, 0);
                    a.b_cond(1, bailout);
                    a.word(0x8a150000); // and x0,x0,x21
                    a.orr(0, 0, 22);
                }
            } else {
                a.movz(2, op as u64);
                call(&mut a, region_binary as *const () as usize);
                guard(&mut a, bailout);
            }
            save(&mut a, dst, bailout);
            if x.op == Op::LocalAddKJ {
                a.b(labels[0]);
            }
            continue;
        }
        match x.op {
            Op::Move | Op::GetLocal | Op::SetLocal => {
                a.ldr(0, 19, x.b * 8);
                save(&mut a, x.a, bailout);
            }
            Op::LoadK => {
                a.mov_const(0, Value::from_f64(consts(x.b as usize)?).0);
                save(&mut a, x.a, bailout);
            }
            Op::LoadTrue | Op::LoadFalse | Op::LoadNull | Op::LoadUndefined => {
                a.mov_const(
                    0,
                    match x.op {
                        Op::LoadTrue => Value::TRUE.0,
                        Op::LoadFalse => Value::FALSE.0,
                        Op::LoadNull => Value::NULL.0,
                        _ => Value::UNDEFINED.0,
                    },
                );
                save(&mut a, x.a, bailout);
            }
            Op::GetProp if virt_get[ip].is_some() => {
                a.ldr(0, 25, virt_get[ip].unwrap() * 8);
                save(&mut a, x.a, bailout);
            }
            Op::GetProp => {
                let cache = unsafe { &*ics.add(ip) };
                let shapes: Vec<(u32, u32)> = match cache.state {
                    1 => vec![(cache.shape, cache.slot)],
                    2 => vec![(cache.shape, cache.slot), (cache.shape2, cache.slot2)],
                    3 => vec![
                        (cache.shape, cache.slot),
                        (cache.shape2, cache.slot2),
                        (cache.shape3, cache.slot3),
                    ],
                    _ => Vec::new(),
                };
                let tuple_size = std::mem::size_of::<(crate::objects::PropKey, Value)>();
                let key_off = std::mem::offset_of!((crate::objects::PropKey, Value), 0);
                let val_off = std::mem::offset_of!((crate::objects::PropKey, Value), 1);
                let expected_key = unsafe {
                    std::ptr::read_unaligned(
                        (&cache.key as *const crate::objects::PropKey).cast::<u64>(),
                    )
                };
                let inline = !shapes.is_empty()
                    && cache.key == unsafe { (*ics.add(ip)).key }
                    && shapes
                        .iter()
                        .all(|(_, slot)| *slot < crate::objects::INLINE_PROPS as u32)
                    && std::mem::size_of::<crate::objects::PropKey>() == 8
                    && tuple_size == 16;
                let slow = a.label();
                let after = a.label();
                if inline {
                    let found: Vec<_> = shapes.iter().map(|_| a.label()).collect();
                    let loaded = a.label();
                    a.ldr(0, 19, x.b * 8);
                    a.ubfx(2, 0, 44, 20);
                    a.mov_const(3, 0x7ff80);
                    a.cmp(2, 3);
                    a.b_cond(1, slow);
                    a.mov_const(3, crate::value::OFFSET_MAX);
                    a.word(0x8a000000 | (3 << 16));
                    a.mov_const(3, arena_base as u64);
                    a.add_reg(0, 0, 3);
                    a.ldr32(
                        2,
                        0,
                        std::mem::offset_of!(crate::objects::ObjHeader, tag) as u32,
                    );
                    a.cmp_imm(2, crate::objects::TAG_OBJECT);
                    a.b_cond(1, slow);
                    a.ldr32(
                        2,
                        0,
                        std::mem::offset_of!(crate::objects::ObjHeader, flags) as u32,
                    );
                    a.mov_const(3, crate::objects::FLAG_ACCESSOR as u64);
                    a.word(0x8a000000 | (3 << 16) | (2 << 5) | 2);
                    a.cbnz(2, slow);
                    a.ldr(
                        2,
                        0,
                        std::mem::offset_of!(crate::objects::JObject, spill) as u32,
                    );
                    a.cbnz(2, slow);
                    for (i, (shape, _slot)) in shapes.iter().enumerate() {
                        a.ldr32(
                            2,
                            0,
                            std::mem::offset_of!(crate::objects::JObject, shape) as u32,
                        );
                        a.mov_const(3, *shape as u64);
                        a.cmp(2, 3);
                        a.b_cond(0, found[i]);
                    }
                    a.b(slow);
                    for (i, (_, slot)) in shapes.iter().enumerate() {
                        a.bind(found[i]);
                        let item = std::mem::offset_of!(crate::objects::JObject, small)
                            + *slot as usize * tuple_size;
                        a.ldr32(
                            2,
                            0,
                            std::mem::offset_of!(crate::objects::JObject, nprops) as u32,
                        );
                        a.cmp_imm(2, *slot + 1);
                        a.b_cond(3, slow);
                        a.ldr(2, 0, (item + key_off) as u32);
                        a.mov_const(3, expected_key);
                        a.cmp(2, 3);
                        a.b_cond(1, slow);
                        a.ldr(0, 0, (item + val_off) as u32);
                        a.b(loaded);
                    }
                    a.bind(loaded);
                    save(&mut a, x.a, bailout);
                    a.b(after);
                }
                a.bind(slow);
                a.ldr(0, 19, x.b * 8);
                a.mov_const(1, unsafe { ics.add(ip) } as u64);
                call(&mut a, region_property as *const () as usize);
                guard(&mut a, bailout);
                save(&mut a, x.a, bailout);
                a.bind(after);
            }
            Op::GetIndex => {
                let slow = a.label();
                let after = a.label();
                let data = std::mem::offset_of!(crate::objects::JArray, data) as u32;
                // Vec on this ABI is capacity, pointer, length.
                const _: () = assert!(std::mem::size_of::<Vec<Value>>() == 24);
                a.ldr(0, 19, x.b * 8);
                a.ldr(1, 19, x.c * 8);
                a.ubfx(2, 0, 44, 20);
                a.mov_const(3, 0x7ff80);
                a.cmp(2, 3);
                a.b_cond(1, slow);
                a.ubfx(2, 1, 44, 20);
                a.cmp(2, 23);
                a.b_cond(1, slow);
                a.sbfx44(1, 1);
                a.tbnz(1, 63, slow);
                a.mov_const(3, crate::value::OFFSET_MAX);
                a.word(0x8a000000 | (3 << 16));
                a.mov_const(3, arena_base as u64);
                a.add_reg(0, 0, 3);
                a.ldr32(2, 0, std::mem::offset_of!(crate::objects::ObjHeader, tag) as u32);
                a.cmp_imm(2, crate::objects::TAG_ARRAY);
                a.b_cond(1, slow);
                a.ldr32(
                    2,
                    0,
                    std::mem::offset_of!(crate::objects::JArray, packed) as u32,
                );
                a.cmp_imm(2, 1);
                a.b_cond(1, slow);
                a.ldr(2, 0, data + 16);
                a.cmp(1, 2);
                a.b_cond(2, slow);
                a.ldr(2, 0, data + 8);
                // add x2, x2, x1, lsl #3
                a.word(0x8B000000 | (1 << 16) | (3 << 10) | (2 << 5) | 2);
                a.ldr(0, 2, 0);
                a.mov_const(3, Value::EMPTY.0);
                a.cmp(0, 3);
                a.b_cond(0, slow);
                a.b(after);
                a.bind(slow);
                a.ldr(0, 19, x.b * 8);
                a.ldr(1, 19, x.c * 8);
                call(&mut a, region_index as *const () as usize);
                guard(&mut a, bailout);
                a.bind(after);
                save(&mut a, x.a, bailout);
            }
            Op::Call | Op::CallThis => {
                if let Some(site) = call_sites.iter().position(|&site| site == ip) {
                    let offset = site as u32 * target_size;
                    let field = |byte: usize| offset + byte as u32;
                    let prepare = a.label();
                    let ready = a.label();
                    a.ldr(16, 24, field(std::mem::offset_of!(RegionCallTarget, entry)));
                    a.cbz(16, prepare);
                    let callee = x.b + if x.op == Op::CallThis { 1 } else { 0 };
                    a.ldr(9, 19, callee * 8);
                    a.ldr(
                        10,
                        24,
                        field(std::mem::offset_of!(RegionCallTarget, callee)),
                    );
                    a.cmp(9, 10);
                    a.b_cond(0, ready);
                    a.bind(prepare);
                    a.ldr(0, 19, callee * 8);
                    a.add_imm(1, 24, offset);
                    call(&mut a, region_prepare_call as *const () as usize);
                    a.cbz(0, bailout);
                    a.bind(ready);
                    let check = a.label();
                    let full = a.label();
                    if x.c == 2 {
                        let arg0 = (x.b + if x.op == Op::CallThis { 2 } else { 1 }) * 8;
                        a.ldr(7, 24, field(std::mem::offset_of!(RegionCallTarget, fold)));
                        a.cbz(7, full);
                        a.ldr(0, 19, arg0);
                        a.ldr(1, 19, arg0 + 8);
                        a.ubfx(2, 0, 44, 20);
                        a.cmp(2, 23);
                        a.b_cond(1, full);
                        a.ubfx(2, 1, 44, 20);
                        a.cmp(2, 23);
                        a.b_cond(1, full);
                        a.sbfx44(0, 0);
                        a.sbfx44(1, 1);
                        let as_mul = a.label();
                        let boxed = a.label();
                        a.cmp_imm(7, 1);
                        a.b_cond(1, as_mul);
                        a.add_reg(0, 0, 1);
                        a.b(boxed);
                        a.bind(as_mul);
                        a.word(0x9b407c02 | (1 << 16));
                        a.eor_regs(5, 0, 1);
                        a.mul_reg(0, 0, 1);
                        a.word(0x937ffc06);
                        a.cmp(2, 6);
                        a.b_cond(1, full);
                        let nz = a.label();
                        a.cbnz(0, nz);
                        a.tbnz(5, 63, full);
                        a.bind(nz);
                        a.bind(boxed);
                        a.sbfx44(2, 0);
                        a.cmp(2, 0);
                        a.b_cond(1, full);
                        a.word(0x8a150000);
                        a.orr(0, 0, 22);
                        a.str_(0, 19, x.a * 8);
                        a.movz(0, 0);
                        a.b(check);
                    }
                    a.bind(full);
                    let args = x.b + if x.op == Op::CallThis { 2 } else { 1 };
                    a.mov_const(0, args as u64 * 8);
                    a.add_reg(0, 19, 0);
                    a.mov_const(1, x.c as u64);
                    a.add_imm(2, 20, 1);
                    a.mov_const(3, x.a as u64 * 8);
                    a.add_reg(3, 19, 3);
                    a.ldr(
                        4,
                        24,
                        field(std::mem::offset_of!(RegionCallTarget, upvalues)),
                    );
                    let receiver = a.label();
                    let invoke = a.label();
                    a.ldr(
                        5,
                        24,
                        field(std::mem::offset_of!(RegionCallTarget, is_arrow)),
                    );
                    a.cbz(5, receiver);
                    a.ldr(
                        5,
                        24,
                        field(std::mem::offset_of!(RegionCallTarget, lexical_this)),
                    );
                    a.b(invoke);
                    a.bind(receiver);
                    if x.op == Op::CallThis {
                        a.ldr(5, 19, x.b * 8);
                    } else {
                        a.mov_const(5, Value::UNDEFINED.0);
                    }
                    a.bind(invoke);
                    let js_call = a.label();
                    a.ldr(6, 24, field(std::mem::offset_of!(RegionCallTarget, native)));
                    a.cbz(6, js_call);
                    a.ldr(7, 24, field(std::mem::offset_of!(RegionCallTarget, callee)));
                    call(&mut a, region_enter_native as *const () as usize);
                    a.b(check);
                    a.bind(js_call);
                    a.ldr(16, 24, field(std::mem::offset_of!(RegionCallTarget, entry)));
                    a.ldr(6, 24, field(std::mem::offset_of!(RegionCallTarget, callee)));
                    a.blr(16);
                    a.bind(check);
                    a.cbnz(0, bailout);
                } else {
                    a.mov_reg(0, 19);
                    a.movz(1, (x.b + if x.op == Op::CallThis { 1 } else { 0 }) as u64);
                    a.movz(2, (x.b + if x.op == Op::CallThis { 2 } else { 1 }) as u64);
                    a.movz(3, x.c as u64);
                    a.movz(4, x.a as u64);
                    a.mov_const(
                        5,
                        if x.op == Op::CallThis {
                            x.b as u64
                        } else {
                            u32::MAX as u64
                        },
                    );
                    a.mov_reg(6, 20);
                    call(&mut a, region_invoke as *const () as usize);
                    a.cbz(0, bailout);
                }
                if cached(x.a).is_some() {
                    a.ldr(0, 19, x.a * 8);
                    save(&mut a, x.a, bailout);
                }
            }
            Op::AppendStr => {
                a.movz(0, x.a as u64);
                a.ldr(1, 19, x.b * 8);
                call(&mut a, region_append as *const () as usize);
                a.cbnz(0, bailout);
            }
            Op::SetProp if virt_skip[ip] => {
                for (src, slot) in &virt_copy[ip] {
                    a.ldr(0, 19, src * 8);
                    a.str_(0, 25, slot * 8);
                }
            }
            Op::SetProp => {
                a.ldr(0, 19, x.a * 8);
                a.ldr(1, 19, x.c * 8);
                a.mov_const(2, unsafe { ics.add(ip) } as u64);
                call(&mut a, region_set_prop as *const () as usize);
                a.cbnz(0, bailout);
            }
            Op::NewObjectLit if virt_skip[ip] => {
                for (src, slot) in &virt_copy[ip] {
                    a.ldr(0, 19, src * 8);
                    a.str_(0, 25, slot * 8);
                }
            }
            Op::NewObjectLit => {
                let consts = REGION_CONSTS.with(|c| c.get());
                a.mov_reg(0, 19);
                a.movz(1, x.a as u64);
                a.movz(2, x.c as u64);
                a.mov_const(3, unsafe { ics.add(ip) } as u64);
                a.mov_const(4, consts as u64);
                a.movz(5, x.b as u64);
                call(&mut a, region_new_lit as *const () as usize);
                a.cbnz(0, bailout);
            }
            Op::Jump => a.b(labels[x.c as usize - top]),
            Op::JumpIfTrue | Op::JumpIfFalse => {
                a.ldr(0, 19, x.a * 8);
                call(&mut a, region_truthy as *const () as usize);
                if x.op == Op::JumpIfTrue {
                    a.cbnz(0, labels[x.c as usize - top]);
                } else {
                    a.cbz(0, labels[x.c as usize - top]);
                }
            }
            _ => return None,
        }
    }
    a.bind(labels[exit - top]);
    a.movz(0, 1);
    let ret = a.label();
    a.b(ret);
    for (idx, label) in bailouts.iter().enumerate() {
        a.bind(*label);
        a.mov_const(0, (top + idx + 2) as u64);
        a.b(ret);
    }
    a.bind(entry_bail);
    a.movz(0, 0);
    a.b(ret);
    a.bind(ret);
    a.add_imm(31, 31, 64);
    if cache_bytes != 0 {
        a.add_imm(31, 31, cache_bytes);
    }
    a.ldp_post(29, 30, 2);
    a.ldp_post(27, 28, 2);
    a.ldp_post(25, 26, 2);
    a.ldp_post(23, 24, 2);
    a.ldp_post(21, 22, 2);
    a.ldp_post(19, 20, 2);
    a.ret();
    let words = a.finish();
    let len = words.len() * 4;
    let code = alloc_exec(len);
    if code.is_null() {
        return None;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(words.as_ptr().cast::<u8>(), code, len);
    }
    if !seal_exec(code, len) {
        unsafe {
            release_exec(code, len);
        }
        return None;
    }
    Some(JitLoop {
        code,
        len,
        exit_ip: exit,
    })
}
