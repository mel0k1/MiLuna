// High Precision Event Timer, ACPI-reported MMIO block (QEMU: 0xFED00000).
// Roles in the kernel:
//   1. reference clock for the per-core LAPIC timer calibration (lapic::
//      calibrate reads the TMCV countdown against HPET milliseconds and
//      derives TICKS_PER_MS -> the TICR value for a preemption slice),
//   2. deadline clock for SYS_SLEEP (per-task wake_ms timestamps).
//
// Layout per the HPET spec (offsets from the block base):
//   0x00 GCAP_ID (RO, 64b): bits 32-63 = counter clock period in
//        femtoseconds (QEMU: 10_000_000 fs == 10 ns == 100 MHz),
//   0x08 GCFG (RW, 64b): bit 0 = ENABLE_CNF (start the main counter),
//        bit 1 = legacy replacement (keep OFF - PIT/RTC stay in charge),
//   0xF0 MCVAL (RO, 64b): main counter value, free-running once enabled.
// The block sits under the 4GB identity map (0..4GB 1GB pages), so plain
// volatile accesses work. All timestamps flow through nanos(): u128
// intermediate so counter * fs can never overflow.

use crate::acpi::HPET_BASE;

static mut FS_PER_TICK: u64 = 0; // 0 = no usable HPET

unsafe fn reg64(off: u64) -> *mut u64 {
    (HPET_BASE + off) as *mut u64
}

unsafe fn reg32(off: u64) -> *mut u32 {
    (HPET_BASE + off) as *mut u32
}

// Find the block, read the clock period, start the main counter. Safe to
// call twice; leaves FS_PER_TICK = 0 on any failure (every accessor then
// falls back to the TSC clock in timer.rs).
//
// Lookup order: ACPI "HPET" table first, then a probe of the architecturally
// fixed block at 0xFED00000 - QEMU wires the HPET into its DSDT AML instead
// of shipping a standalone HPET SSDT, so the table lookup comes up empty
// there even though the device is present and fully functional.
//
// QEMU register-offset quirk: the spec puts the General Configuration
// register at +0x8, but QEMU's hw/timer/hpet.h defines HPET_CFG as 0x10 and
// its MMIO dispatch ignores +0x8 entirely - an enable write following the
// spec is silently LOST under QEMU (found with -d trace:hpet_ram_write: the
// write reaches the device and vanishes). Write the enable bit through BOTH
// offsets: on real hardware +0x8 is the real GCFG and +0x10 is the W1C
// interrupt-status register (writing 1 just clears pending bits), under
// QEMU +0x8 is dead and +0x10 is the actual GCFG.
pub unsafe fn init() {
    if FS_PER_TICK != 0 {
        return;
    }
    let mut base = crate::acpi::hpet_base();
    if base == 0 {
        base = 0xFED0_0000;
    }
    crate::acpi::HPET_BASE = base;
    let cap = reg64(0x00).read_volatile();
    let fs = cap >> 32;
    let rev = cap & 0xFF;
    // sanity: revision must exist and the tick period must be within
    // 1 ns .. 1 ms (100 MHz QEMU reports 10_000_000 fs; unmapped MMIO
    // reads back all-ones and fails the fs check)
    if rev == 0 || fs < 1_000_000 || fs > 1_000_000_000_000 {
        crate::acpi::HPET_BASE = 0;
        return;
    }
    FS_PER_TICK = fs;
    // enable the main counter, legacy replacement off (bit 1 stays 0), and
    // verify it actually runs - a dead block must not back our deadlines
    let cfg8 = reg32(0x08).read_volatile();
    let cfg10 = reg32(0x10).read_volatile();
    reg32(0x08).write_volatile(cfg8 | 1);  // spec GCFG (real hardware)
    reg32(0x10).write_volatile(cfg10 | 1); // QEMU's HPET_CFG offset
    let c0 = counter();
    let mut spin = 0u64;
    while spin < 200_000 {
        core::hint::spin_loop();
        spin += 1;
    }
    if counter() == c0 {
        // counter stuck: unusable, fall back to the TSC clock
        reg32(0x08).write_volatile(cfg8);
        reg32(0x10).write_volatile(cfg10);
        FS_PER_TICK = 0;
        crate::acpi::HPET_BASE = 0;
        return;
    }
    // debugcon: "H" + hex fs-per-tick - the boot log records the HPET block
    // and its clock period (QEMU: H00989680 -> 10_000_000 fs = 100 MHz)
    let mut buf = [0u8; 9];
    buf[0] = b'H';
    let mut shift = 28i32;
    let mut n = 1;
    while shift >= 0 {
        let d = ((fs >> shift) & 0xF) as u8;
        buf[n] = if d < 10 { b'0' + d } else { b'A' + d - 10 };
        n += 1;
        shift -= 4;
    }
    for i in 0..n {
        core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") buf[i], options(nomem, nostack));
    }
}

pub fn available() -> bool {
    unsafe { core::ptr::read_volatile(&raw const FS_PER_TICK) != 0 }
}

// raw main counter (0 when no HPET)
pub fn counter() -> u64 {
    unsafe {
        if !available() {
            return 0;
        }
        reg64(0xF0).read_volatile()
    }
}

// nanoseconds on the HPET clock (TSC microseconds when no HPET exists)
pub fn nanos() -> u64 {
    unsafe {
        let fs = core::ptr::read_volatile(&raw const FS_PER_TICK);
        if fs == 0 {
            return crate::timer::millis() * 1_000_000;
        }
        let c = reg64(0xF0).read_volatile();
        // femtoseconds -> nanoseconds: divide by 1_000_000 (NOT 1_000 - an
        // earlier /1_000 returned picoseconds, pushing millis() back into
        // microseconds and shrinking every HPET interval by 1000x again)
        ((c as u128 * fs as u128) / 1_000_000) as u64
    }
}

// NOTE: /1_000_000 here, NOT /1_000 - an earlier version divided nanoseconds
// by 1000 and returned MICROseconds, which silently shrank every HPET-based
// interval (calibration window, sleep deadlines, sweep rate limits) by 1000x
// and made the first calibration read ~120 instead of 62500 ticks per ms.
pub fn millis() -> u64 {
    nanos() / 1_000_000
}
