// SMP bringup: copy the trampoline to 0x8000, patch per-AP parameters,
// wake each AP with INIT + SIPI, wait for it to report online.
// Phase 1: APs enter long mode, load their own TSS and park in a hlt loop;
// per-CPU scheduling is the next roadmap step.

use core::sync::atomic::{AtomicBool, Ordering};

const TRAMP_ADDR: u64 = 0x8000;
const TRAMP_PATCH: u64 = TRAMP_ADDR + 0x100;
pub const MAX_APS: usize = 4;

static TRAMPOLINE_BIN: &[u8] = include_bytes!("../target/tramp.bin");

static ONLINE: [AtomicBool; MAX_APS] = [
    AtomicBool::new(false),
    AtomicBool::new(false),
    AtomicBool::new(false),
    AtomicBool::new(false),
];

#[repr(align(16))]
#[derive(Clone, Copy)]
struct ApStack([u8; 4096]);
static mut AP_STACKS: [ApStack; MAX_APS] = [ApStack([0; 4096]); MAX_APS];

// Per-AP TSS and private RSP0 stacks (APs don't run ring 3 yet, but the
// structures are ready for per-CPU scheduling).
#[repr(align(16))]
#[derive(Clone, Copy)]
struct ApRsp0([u8; 8192]);
static mut AP_RSP0: [ApRsp0; MAX_APS] = [ApRsp0([0; 8192]); MAX_APS];

static mut AP_COUNT: usize = 0;
static mut AP_IDS: [u8; MAX_APS] = [0; MAX_APS];

unsafe fn wr_u16(a: u64, v: u16) { core::ptr::write_volatile(a as *mut u16, v) }
unsafe fn wr_u64(a: u64, v: u64) { core::ptr::write_unaligned(a as *mut u64, v) }

// AP entry point, called from the trampoline in long mode with interrupts off
pub extern "C" fn smp_ap_entry(apic_id: u64) -> ! {
    for i in 0..MAX_APS {
        if unsafe { AP_IDS[i] } as u64 == apic_id {
            ONLINE[i].store(true, Ordering::Release);
            break;
        }
    }
    loop {
        unsafe {
            core::arch::asm!("cli", "hlt", options(nomem, nostack));
        }
    }
}

pub fn online_count() -> usize {
    let mut n = 0;
    for i in 0..MAX_APS {
        if ONLINE[i].load(Ordering::Acquire) {
            n += 1;
        }
    }
    unsafe { n + 1 } // + BSP
}

pub fn ap_ids() -> [u8; MAX_APS] {
    unsafe { AP_IDS }
}

pub unsafe fn init() {
    let cpus = crate::acpi::CPU_COUNT;
    if cpus <= 1 {
        return;
    }
    // park area must be RAM: copy the 16-bit entry code
    core::ptr::copy_nonoverlapping(
        TRAMPOLINE_BIN.as_ptr(),
        TRAMP_ADDR as *mut u8,
        TRAMPOLINE_BIN.len(),
    );

    let bsp = crate::acpi::bsp_apic_id();
    let mut targets: [u8; MAX_APS] = [0; MAX_APS];
    let mut ntargets = 0usize;
    for i in 0..cpus {
        let id = crate::acpi::APIC_IDS[i];
        if id == bsp || ntargets >= MAX_APS {
            continue;
        }
        targets[ntargets] = id;
        ntargets += 1;
    }

    // register per-AP TSS descriptors in the shared GDT (selectors 0x40..0x70)
    for i in 0..ntargets {
        crate::gdt::write_ap_tss(i, AP_RSP0[i].0.as_ptr() as u64 + 8192);
    }
    crate::gdt::reload();

    // patch + boot each AP sequentially (wait for online before the next)
    let entry = smp_ap_entry as u64;
    for i in 0..ntargets {
        let id = targets[i];
        AP_IDS[i] = id;
        let p = TRAMP_PATCH;
        // kgdt: [limit u16][base u64] contiguous, read by lgdt
        let (glim, gbase) = crate::gdt::gdtr_values();
        wr_u16(p, glim);
        wr_u64(p + 2, gbase);
        wr_u16(p + 0xA, 0x40 + (i as u16) * 0x10); // ktss_sel
        wr_u64(p + 0x10, (AP_STACKS[i].0.as_ptr() as u64) + 4096); // kstack
        wr_u64(p + 0x18, id as u64); // kapic
        wr_u64(p + 0x20, entry); // kentry
        core::sync::atomic::compiler_fence(Ordering::SeqCst);

        crate::lapic::bringup(id, 0x08);
        let mut waited = 0u32;
        while !ONLINE[i].load(Ordering::Acquire) && waited < 2_000_000 {
            core::arch::asm!("pause", options(nomem, nostack));
            waited += 1;
        }
    }
    AP_COUNT = ntargets;
}

pub fn detected() -> usize {
    unsafe { AP_COUNT }
}
