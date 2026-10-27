// Local APIC: MMIO registers, enable, EOI, INIT/SIPI IPIs for SMP bringup
// and the per-CPU periodic timer that drives the scheduler on every core.

pub unsafe fn reg(off: u32) -> *mut u32 {
    // acpi::init may not have run yet (early timer ticks) - fall back to the
    // standard LAPIC mapping
    let base = if crate::acpi::LAPIC_BASE != 0 {
        crate::acpi::LAPIC_BASE
    } else {
        0xFEE00000
    };
    (base + off as u64) as *mut u32
}

pub unsafe fn enable() {
    // spurious interrupt register: vector 0xFF + APIC software enable (bit 8)
    reg(0xF0).write_volatile(0x1FF);
}

pub unsafe fn id() -> u8 {
    (reg(0x20).read_volatile() >> 24) as u8
}

pub unsafe fn eoi() {
    reg(0xB0).write_volatile(0);
}

// BSP: calibrate the LAPIC timer against the TSC (millis) and start it in
// periodic mode at 50 Hz, then retire the PIT as the tick source.
// The resulting interval is global - QEMU runs identical LAPIC timers on
// every core, so APs just program the same initial count.
pub static mut TIMER_INIT_COUNT: u32 = 0;
// real LAPIC timer interrupt arrivals, bumped in the tick handler; used for
// the runtime rate correction (QEMU's LAPIC decay does not match its own
// calibration-time decay, so the interval is re-derived from observed ticks)
pub static mut TICK_ARRIVALS: u32 = 0;

pub unsafe fn timer_init_bsp() {
    reg(0x3E0).write_volatile(0b1000); // divide by 16
    reg(0x320).write_volatile(0x0003_0020); // vector 0x20 | mask | periodic
    reg(0x380).write_volatile(0xFFFF_FFFFu32);
    let t0 = crate::timer::millis();
    let mut t1 = t0;
    while t1.wrapping_sub(t0) < 40 {
        t1 = crate::timer::millis();
    }
    let cur = reg(0x390).read_volatile();
    let elapsed = 0xFFFF_FFFFu32.wrapping_sub(cur);
    let per_20ms = (elapsed / 2).max(16);
    TIMER_INIT_COUNT = per_20ms;
    reg(0x380).write_volatile(per_20ms);
    reg(0x320).write_volatile(0x0002_0020); // unmask: vector | periodic
    eoi();
    // the PIT's IRQ0 is retired: the LAPIC timer drives all scheduling now
    crate::pit::mask_irq0();
    // runtime correction: count actual arrivals over one RTC second and
    // re-derive the interval - QEMU's LAPIC decays at a different rate once
    // the calibration window closes, so trust the observed ticks, not TSC
    core::ptr::write_volatile(&raw mut TICK_ARRIVALS, 0);
    crate::timer::wait_second_boundary();
    let a0 = core::ptr::read_volatile(&raw const TICK_ARRIVALS);
    crate::timer::wait_second_boundary();
    let a1 = core::ptr::read_volatile(&raw const TICK_ARRIVALS);
    let observed = a1.wrapping_sub(a0);
    if observed > 5 {
        let corrected = ((per_20ms as u64) * (observed as u64) / 50).clamp(16, 0xFFFFFFF0) as u32;
        TIMER_INIT_COUNT = corrected;
        reg(0x380).write_volatile(corrected);
        eoi();
    }
}

// AP: program the timer with the BSP-calibrated interval
pub unsafe fn timer_start_ap() {
    let c = core::ptr::read_volatile(&raw const TIMER_INIT_COUNT);
    reg(0x3E0).write_volatile(0b1000);
    reg(0x320).write_volatile(0x0002_0020);
    reg(0x380).write_volatile(c);
}

fn io_wait(n: u32) {
    let mut i = 0;
    while i < n {
        unsafe {
            core::arch::asm!(
                "out 0x80, al",
                in("al") 0u8,
                options(nomem, nostack, preserves_flags)
            );
        }
        i += 1;
    }
}

pub unsafe fn send_init(dst: u8) {
    reg(0x310).write_volatile((dst as u32) << 24);
    // delivery mode 101 (INIT), level assert
    reg(0x300).write_volatile(0x0000_4500);
}

pub unsafe fn send_sipi(dst: u8, vector: u8) {
    reg(0x310).write_volatile((dst as u32) << 24);
    // delivery mode 110 (STARTUP), level assert
    reg(0x300).write_volatile(0x0000_4600 | vector as u32);
}

// INIT -> wait -> SIPI x2, the classic bringup sequence
pub unsafe fn bringup(dst: u8, vector: u8) -> bool {
    send_init(dst);
    io_wait(8000); // ~10 ms of port-0x80 delays
    send_sipi(dst, vector);
    io_wait(400);
    send_sipi(dst, vector);
    io_wait(2000);
    true
}
