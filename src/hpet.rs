// High Precision Event Timer: ACPI HPET table -> MMIO counter at (typically)
// 0xFED00000. Serves as the kernel's monotonic time source: nanosecond
// counter, no per-boot calibration dance (the old RTC-UIP TSC measurement
// burned ~2s at boot and drifted under QEMU's virtual TSC). Also backs the
// LAPIC timer calibration (lapic::calibrate) and SYS_SLEEP deadlines.
//
// Registers (offsets from BASE):
//   0x00/0x04 GeneralCapabilitiesAndId: [63:32] counter period in femtoseconds
//   0x10     GeneralConfiguration: bit0 ENABLE_CNF, bit1 legacy replacement
//   0xF0/0xF4 MainCounterValue low/high (64-bit, free running)
//
// NOTE on the GCFG offset: the HPET spec places the General Configuration
// register at +0x8, but QEMU's hw/timer/hpet.h defines HPET_CFG as 0x10 and
// its MMIO dispatch silently ignores +0x8 - a spec-conforming enable write
// is LOST under QEMU (found with -d trace:hpet_ram_write: the write reaches
// the device and vanishes). On real hardware +0x10 is the W1C interrupt
// status register, so we probe both: enable through +0x10 (QEMU and the
// real GCFG live there in practice for QEMU/SeaBIOS machines), and validate
// that the counter actually runs before trusting this block.
//
// We enable the counter but keep legacy replacement OFF: the PIT stays the
// IRQ0/wall-clock interrupt source, HPET only provides precise elapsed time.

pub static mut BASE: u64 = 0;
static mut PERIOD_FS: u64 = 0; // femtoseconds per counter tick (0 = absent)

unsafe fn reg(off: u32) -> *mut u32 {
    (BASE + off as u64) as *mut u32
}

pub fn available() -> bool {
    unsafe { BASE != 0 && PERIOD_FS != 0 }
}

// femtoseconds per tick as reported by the capabilities register
pub fn period_fs() -> u64 {
    unsafe { core::ptr::read_volatile(&raw const PERIOD_FS) }
}

// counter frequency in Hz (1e15 fs = 1 s)
pub fn freq_hz() -> u64 {
    let p = period_fs();
    if p == 0 {
        0
    } else {
        1_000_000_000_000_000u64 / p
    }
}

pub unsafe fn init(base: u64) {
    if base == 0 {
        return;
    }
    BASE = base;
    let caps = reg(0x00).read_volatile() as u64 | ((reg(0x04).read_volatile() as u64) << 32);
    let period = caps >> 32;
    // sanity: the tick period must be within 1 ns .. 1 ms. An unmapped or
    // dead block reads back all-ones / zero and fails this check - without
    // it a bogus PERIOD_FS would poison every derived timestamp.
    if period < 1_000_000 || period > 1_000_000_000_000 {
        BASE = 0;
        return;
    }
    PERIOD_FS = period;
    // reset the main counter, then enable counting (legacy replacement off)
    let cfg = reg(0x10).read_volatile();
    reg(0x10).write_volatile(cfg & !0x1);
    reg(0xF0).write_volatile(0);
    reg(0xF4).write_volatile(0);
    reg(0x10).write_volatile(cfg | 0x1);
    // verify the counter actually runs - a frozen block must not back the
    // LAPIC calibration or sleep deadlines (fall back to the TSC clock)
    let c0 = counter();
    let mut spin = 0u64;
    while spin < 200_000 {
        core::hint::spin_loop();
        spin += 1;
    }
    if counter() == c0 {
        reg(0x10).write_volatile(cfg & !0x1);
        BASE = 0;
        PERIOD_FS = 0;
        return;
    }
}

// raw 64-bit main counter; two 32-bit halves with the classic rollover retry
pub fn counter() -> u64 {
    unsafe {
        if BASE == 0 {
            return 0;
        }
        loop {
            let hi1 = reg(0xF4).read_volatile() as u64;
            let lo = reg(0xF0).read_volatile() as u64;
            let hi2 = reg(0xF4).read_volatile() as u64;
            if hi1 == hi2 {
                return (hi1 << 32) | lo;
            }
        }
    }
}

// elapsed nanoseconds since hpet::init (u128 intermediate: period up to
// ~0.1s/tick would overflow u64 after minutes otherwise)
pub fn nanos() -> u64 {
    let p = period_fs();
    if p == 0 {
        return 0;
    }
    let t = (counter() as u128) * (p as u128) / 1_000_000u128;
    t as u64
}

pub fn millis() -> u64 {
    // nanoseconds -> milliseconds: /1_000_000. Dividing by 1_000 (an easy
    // slip both authoring passes made independently) yields MICROseconds,
    // which silently compresses every timer in the system - the LAPIC
    // calibration window, DHCP/net timeouts, sleep deadlines, uptime - by
    // 1000x while the raw counter stays correct, so it hides in testing.
    nanos() / 1_000_000
}
