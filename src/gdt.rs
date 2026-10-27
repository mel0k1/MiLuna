// GDT with user-mode segments and TSS for ring 3 support
// Selector values match boot.asm: 0x08 kcode, 0x10 kdata, 0x18 ucode, 0x20 udata, 0x28 TSS

pub const USER_CS: u16 = 0x1B;
pub const USER_SS: u16 = 0x23;
pub const TSS_SEL: u16 = 0x28;

pub const KERNEL_CODE: u64 = 0x00209A000000FFFF;
pub const KERNEL_DATA: u64 = 0x002092000000FFFF;
pub const USER_CODE: u64 = 0x0020FA000000FFFF;
pub const USER_DATA: u64 = 0x0020F2000000FFFF;

// Hardware layout: RSP0 at offset 4 (unaligned), total 104 bytes
#[repr(align(16))]
pub struct Tss([u8; 104]);

pub static mut TSS: Tss = Tss([0; 104]);

impl Tss {
    pub fn set_rsp0(&mut self, v: u64) {
        self.0[4..12].copy_from_slice(&v.to_le_bytes());
    }
}

#[repr(C, align(16))]
struct GdtTable([u64; 8]);

static mut GDT: GdtTable = GdtTable([0; 8]);

#[repr(C, align(16))]
struct Rsp0Area([u8; 8192]);

#[unsafe(no_mangle)]
pub static mut RSP0_STACK: Rsp0Area = Rsp0Area([0; 8192]);

#[repr(C, packed)]
struct Gdtr {
    limit: u16,
    base: u64,
}

pub unsafe fn init() {
    let g = core::ptr::addr_of_mut!(GDT);
    let w = |i: usize, v: u64| {
        core::ptr::write_volatile(core::ptr::addr_of_mut!((*g).0[i]), v);
    };
    w(1, KERNEL_CODE);
    w(2, KERNEL_DATA);
    w(3, USER_CODE);
    w(4, USER_DATA);

    let tss_addr = core::ptr::addr_of!(TSS) as u64;
    let limit = (core::mem::size_of::<Tss>() - 1) as u64;
    let lo = (limit & 0xFFFF)
        | ((tss_addr & 0xFFFFFF) << 16)
        | (0x89u64 << 40)
        | (((tss_addr >> 24) & 0xFF) << 56);
    let hi = tss_addr >> 32;
    w(5, lo);
    w(6, hi);
    w(7, 0);

    TSS.set_rsp0(core::ptr::addr_of!(RSP0_STACK) as u64 + 8192);
    let gdtr = Gdtr {
        limit: (8 * 8 - 1) as u16,
        base: g as u64,
    };
    let sel: u16 = TSS_SEL;
    core::arch::asm!(
        "lgdt [{p}]",
        "mov ax, {sel:x}",
        "ltr ax",
        p = in(reg) &gdtr as *const Gdtr,
        sel = in(reg) sel,
        out("ax") _,
    );
}
