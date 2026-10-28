use core::arch::x86_64::_rdtsc;

static mut TSC_PER_MS: u64 = 0;
static mut TSC_START: u64 = 0;

fn inb(port: u16) -> u8 {
    let v: u8;
    unsafe { core::arch::asm!("in al, dx", in("dx") port, out("al") v, options(nomem, nostack)) }
    v
}
fn outb(port: u16, val: u8) {
    unsafe { core::arch::asm!("out dx, al", in("dx") port, in("al") val, options(nomem, nostack)) }
}

unsafe fn rtc_uip() -> bool {
    outb(0x70, 0x0A);
    inb(0x71) & 0x80 != 0
}

// Wait for one full RTC second: UIP falls exactly at the second boundary.
// The PC-speaker TSC calibration used here before ran ~2.2x slow under QEMU,
// which skewed millis() and every timer derived from it.
unsafe fn rtc_wait_second_boundary() {
    let mut guard = 0u64;
    while rtc_uip() {
        guard += 1;
        if guard > 100_000_000 { return; }
    }
    guard = 0;
    while !rtc_uip() {
        guard += 1;
        if guard > 100_000_000 { return; }
    }
    guard = 0;
    while rtc_uip() {
        guard += 1;
        if guard > 4_000_000 { return; }
    }
}

// public wrapper: exact one-second wait used by the LAPIC timer calibration
pub fn wait_second_boundary() {
    unsafe { rtc_wait_second_boundary() }
}

pub fn rdtsc() -> u64 { unsafe { _rdtsc() } }

pub fn init() {
    unsafe {
        if TSC_PER_MS != 0 { return; }
        // measure the TSC over exactly one RTC second (UIP edges)
        rtc_wait_second_boundary();
        let t0 = rdtsc();
        rtc_wait_second_boundary();
        let t1 = rdtsc();
        let best = (t1 - t0) / 1000;
        if best < 1000 { TSC_PER_MS = 1_000_000; } else { TSC_PER_MS = best; }
        TSC_START = rdtsc();
    }
}

pub fn millis() -> u64 {
    unsafe {
        if TSC_PER_MS == 0 { init(); }
        (rdtsc() - TSC_START) / TSC_PER_MS
    }
}

pub fn sleep_ms(ms: u64) {
    let end = millis() + ms;
    while millis() < end { core::hint::spin_loop(); }
}
