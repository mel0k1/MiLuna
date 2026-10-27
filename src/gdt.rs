// GDT with user-mode segments, TSS and syscall/sysret MSRs.
// Selector layout is chosen for SYSRET: it loads CS = STAR[63:48]+16 and
// SS = STAR[63:48]+8, so user data must sit 8 below user code.
//   0x08 kcode | 0x10 kdata | 0x18 udata | 0x20 ucode | 0x30 TSS (BSP)
// Entries 8..15 are reserved for per-AP TSS descriptors (0x40..0x70).

pub const USER_CS: u16 = 0x23; // selector 0x20 | RPL 3
pub const USER_SS: u16 = 0x1B; // selector 0x18 | RPL 3
pub const TSS_SEL: u16 = 0x30;

pub const KERNEL_CODE: u64 = 0x00209A000000FFFF;
pub const KERNEL_DATA: u64 = 0x002092000000FFFF;
pub const USER_CODE: u64 = 0x0020FA000000FFFF;
pub const USER_DATA: u64 = 0x0020F2000000FFFF;

// Hardware layout: RSP0 at offset 4 (unaligned), total 104 bytes
#[repr(align(16))]
#[derive(Clone, Copy)]
pub struct Tss([u8; 104]);

pub static mut TSS: Tss = Tss([0; 104]);

impl Tss {
    pub fn set_rsp0(&mut self, v: u64) {
        self.0[4..12].copy_from_slice(&v.to_le_bytes());
    }
}

#[repr(C, align(16))]
struct GdtTable([u64; 16]);

static mut GDT: GdtTable = GdtTable([0; 16]);

#[repr(C, align(16))]
pub struct Rsp0Area([u8; 8192]);

#[unsafe(no_mangle)]
pub static mut RSP0_STACK: Rsp0Area = Rsp0Area([0; 8192]);

#[repr(C, packed)]
pub struct Gdtr {
    pub limit: u16,
    pub base: u64,
}

pub static mut GDTR: Gdtr = Gdtr { limit: 0, base: 0 };

pub const MAX_APS: usize = 4;
pub static mut AP_TSS: [Tss; MAX_APS] = [Tss([0; 104]); MAX_APS];

pub unsafe fn init() {
    let g = core::ptr::addr_of_mut!(GDT);
    let w = |i: usize, v: u64| {
        core::ptr::write_volatile(core::ptr::addr_of_mut!((*g).0[i]), v);
    };
    w(1, KERNEL_CODE);
    w(2, KERNEL_DATA);
    w(3, USER_DATA);
    w(4, USER_CODE);

    let tss_addr = core::ptr::addr_of!(TSS) as u64;
    let limit = (core::mem::size_of::<Tss>() - 1) as u64;
    let lo = (limit & 0xFFFF)
        | ((tss_addr & 0xFFFFFF) << 16)
        | (0x89u64 << 40)
        | (((tss_addr >> 24) & 0xFF) << 56);
    let hi = tss_addr >> 32;
    w(6, lo);
    w(7, hi);

    TSS.set_rsp0(core::ptr::addr_of!(RSP0_STACK) as u64 + 8192);
    GDTR.limit = (16 * 8 - 1) as u16;
    GDTR.base = g as u64;
    let sel: u16 = TSS_SEL;
    core::arch::asm!(
        "lgdt [{p}]",
        "mov ax, {sel:x}",
        "ltr ax",
        p = in(reg) core::ptr::addr_of!(GDTR),
        sel = in(reg) sel,
        out("ax") _,
    );
}

// write a TSS descriptor for AP i into reserved GDT slots (selector 0x40+0x10*i)
pub unsafe fn write_ap_tss(i: usize, rsp0_top: u64) {
    if i >= MAX_APS {
        return;
    }
    AP_TSS[i].set_rsp0(rsp0_top);
    let g = core::ptr::addr_of_mut!(GDT);
    let tss_addr = core::ptr::addr_of!(AP_TSS[i]) as u64;
    let limit = (core::mem::size_of::<Tss>() - 1) as u64;
    let lo = (limit & 0xFFFF)
        | ((tss_addr & 0xFFFFFF) << 16)
        | (0x89u64 << 40)
        | (((tss_addr >> 24) & 0xFF) << 56);
    let hi = tss_addr >> 32;
    core::ptr::write_volatile(core::ptr::addr_of_mut!((*g).0[8 + 2 * i]), lo);
    core::ptr::write_volatile(core::ptr::addr_of_mut!((*g).0[9 + 2 * i]), hi);
}

// re-load the GDT pointer after AP descriptors were added. TR survives lgdt
// and must NOT be re-loaded: the BSP TSS descriptor is busy (LTR marks it 0xB)
// and loading a busy TSS raises #GP(0x30).
pub unsafe fn reload() {
    core::arch::asm!(
        "lgdt [{p}]",
        p = in(reg) core::ptr::addr_of!(GDTR),
    );
}

pub unsafe fn gdtr_values() -> (u16, u64) {
    (GDTR.limit, GDTR.base)
}

fn wrmsr(msr: u32, val: u64) {
    unsafe {
        core::arch::asm!(
            "wrmsr",
            in("ecx") msr,
            in("eax") val as u32,
            in("edx") (val >> 32) as u32,
            options(nomem, nostack, preserves_flags),
        );
    }
}

// Program the syscall/sysret fast path MSRs. EFER.SCE is already set by boot.asm;
// STAR: syscall enters CS=0x08/SS=0x10, sysret leaves to CS=0x20/SS=0x18 (RPL3 forced).
// SFMASK clears TF|IF|DF|NT on entry so the entry window runs with interrupts off.
pub unsafe fn init_syscall() {
    let star: u64 = (0x10u64 << 48) | (0x08u64 << 32);
    wrmsr(0xC000_0081, star);
    wrmsr(0xC000_0082, crate::trap::syscall_fast_entry as u64);
    wrmsr(0xC000_0084, 0x4700);
}
