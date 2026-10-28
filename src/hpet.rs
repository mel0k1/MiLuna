// High Precision Event Timer: ACPI HPET table -> MMIO counter at (typically)
// 0xFED00000. Serves as the kernel's monotonic time source: nanosecond
// counter, no per-boot calibration dance (the old RTC-UIP TSC measurement
// burned ~2s at boot and drifted under QEMU's virtual TSC).
//
// Registers (offsets from BASE):
//   0x00/0x04 GeneralCapabilitiesAndId: [63:32] counter period in femtoseconds
//   0x10     GeneralConfiguration: bit0 ENABLE_CNF, bit1 legacy replacement
//   0xF0/0xF4 MainCounterValue low/high (64-bit, free running)
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
    if period == 0 {
        // broken/absent timer block: fall back to the TSC path
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
    nanos() / 1000
}
