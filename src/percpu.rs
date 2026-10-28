// Per-CPU state, addressed via the GS segment (IA32_GS_BASE per core).
// The kernel never uses GS for anything else and never runs swapgs, so
// gs:[off] is unambiguous on every core. Layout MUST match the offsets
// used in trap.rs naked asm:
//   +0  self (points at this PerCpu, gs:[0] == gs base)
//   +8  cur  (*mut Task running on this core)
//   +16 cpu  (cpu index: 0 = BSP, 1.. = APs)
//   +24 apic (APIC id)
//   +32 ticks (LAPIC timer ticks on this core)
//   +40 ipis  (resched IPIs received)
//   +48 steals (tasks stolen from other cores)
//   +56 switches (context switches)
//   +64 scratch (syscall entry: user rsp transit, per-core racing-safe)

#[repr(C, align(16))]
#[derive(Clone, Copy)]
pub struct PerCpu {
    pub selfp: u64,
    pub cur: u64,
    pub cpu: u64,
    pub apic: u64,
    pub ticks: u64,
    pub ipis: u64,
    pub steals: u64,
    pub switches: u64,
    pub scratch: u64,
    pub pad: [u64; 8],
}

pub const MAX_CPU: usize = 5;

pub static mut CPUS: [PerCpu; MAX_CPU] = [PerCpu {
    selfp: 0, cur: 0, cpu: 0, apic: 0, ticks: 0, ipis: 0, steals: 0,
    switches: 0, scratch: 0, pad: [0; 8],
}; MAX_CPU];

pub static mut ONLINE: usize = 1; // BSP only until APs report in

unsafe fn wrmsr(msr: u32, val: u64) {
    core::arch::asm!(
        "wrmsr",
        in("ecx") msr,
        in("eax") val as u32,
        in("edx") (val >> 32) as u32,
        options(nomem, nostack, preserves_flags),
    );
}

// BSP setup: cpu 0. Called before task::init (which stores the first cur).
pub unsafe fn init() {
    let base = core::ptr::addr_of_mut!(CPUS[0]) as u64;
    CPUS[0].selfp = base;
    CPUS[0].cpu = 0;
    CPUS[0].apic = crate::acpi::bsp_apic_id() as u64;
    wrmsr(0xC000_0101, base); // IA32_GS_BASE
}

// AP setup: called from smp_ap_entry with interrupts still off.
pub unsafe fn init_ap(idx: usize, apic_id: u64) {
    if idx >= MAX_CPU {
        return;
    }
    let base = core::ptr::addr_of_mut!(CPUS[idx]) as u64;
    CPUS[idx].selfp = base;
    CPUS[idx].cpu = idx as u64;
    CPUS[idx].apic = apic_id;
    CPUS[idx].cur = 0;
    wrmsr(0xC000_0101, base);
    ONLINE += 1;
}

#[inline]
pub fn cpu_idx() -> usize {
    let v: u64;
    unsafe {
        core::arch::asm!("mov {}, gs:16", out(reg) v, options(nostack, preserves_flags));
    }
    v as usize
}

#[inline]
pub fn apic_id() -> u64 {
    let v: u64;
    unsafe {
        core::arch::asm!("mov {}, gs:24", out(reg) v, options(nostack, preserves_flags));
    }
    v
}

#[inline]
pub fn cur() -> *mut crate::task::Task {
    let v: u64;
    unsafe {
        core::arch::asm!("mov {}, gs:8", out(reg) v, options(nostack, preserves_flags));
    }
    v as *mut crate::task::Task
}

#[inline]
pub fn cur_set(t: *mut crate::task::Task) {
    let v = t as u64;
    unsafe {
        core::arch::asm!("mov gs:8, {}", in(reg) v, options(nostack, preserves_flags));
    }
}

#[inline]
pub fn cur_of(idx: usize) -> *mut crate::task::Task {
    unsafe { CPUS[idx].cur as *mut crate::task::Task }
}

pub fn online() -> usize {
    unsafe { core::ptr::read_volatile(&raw const ONLINE) }
}

// timer ticks accumulated on the current core (calibration counter)
pub fn ticks_now() -> u64 {
    unsafe {
        let idx = cpu_idx();
        core::ptr::read_volatile(&raw const CPUS[idx].ticks)
    }
}

pub fn bump(field: u32) {
    unsafe {
        let idx = cpu_idx();
        let c = core::ptr::addr_of_mut!(CPUS[idx]);
        match field {
            0 => (*c).ticks += 1,
            1 => (*c).ipis += 1,
            2 => (*c).steals += 1,
            _ => (*c).switches += 1,
        }
    }
}
