// Local APIC: MMIO registers, enable, EOI, INIT/SIPI IPIs for SMP bringup,
// per-core timer preemption and fixed IPIs (0x31 = resched) for load-steal.
//
// QEMU TCG quirk: MMIO writes issued from LAPIC-interrupt context are
// dropped, so a periodic LAPIC timer + EOI-in-handler deadlocks after the
// first tick (ISR/IRR stuck, PPR raised). Workaround: run the timer in
// ONE-SHOT mode and re-arm it from TASK context (tick_service) on an HPET
// clock - task-context MMIO writes land reliably. tick_service is called
// from the GUI main loop, from every syscall and from the idle loops, so
// every active core keeps a steady ~100 Hz preemption stream, and the sweep
// effect of the EOI also unsticks any pending IPI delivery.
//
// The one-shot period is HPET-CALIBRATED: calibrate() counts how far the
// TMCV down-counter runs during a known 30 ms HPET interval and stores
// TICKS_PER_MS; arm_slice() then loads TICR = TICKS_PER_MS * SLICE_MS, so a
// slice is a real 10 ms on every machine (no more fixed 62500 guess that
// assumed a 1 GHz APIC bus).

pub const VEC_TIMER: u8 = 0x24;   // per-core LAPIC timer -> schedule() (0x21 is IRQ1 keyboard!)
pub const VEC_RESCHED: u8 = 0x31; // inter-core "look at the run queue" IPI

// Preemption slice handed to every task, in milliseconds of HPET time.
pub const SLICE_MS: u64 = 10;

// LAPIC timer down-counter ticks (after the /16 divider) per millisecond,
// measured by calibrate() against the HPET. 0 = not calibrated yet.
pub static mut TICKS_PER_MS: u64 = 0;

pub unsafe fn reg(off: u32) -> *mut u32 {
    (crate::acpi::LAPIC_BASE + off as u64) as *mut u32
}

pub unsafe fn enable() {
    // spurious interrupt register: vector 0xFF + APIC software enable (bit 8)
    reg(0xF0).write_volatile(0x1FF);
}

// Measure this core's LAPIC timer frequency against the HPET: load the
// one-shot with a full 32-bit count, sample TMCV (0x390) 30 ms of HPET time
// apart, derive ticks-per-ms (post-divider). Called once on the BSP before
// the APs come up; every core's APIC bus runs at the same frequency.
pub unsafe fn calibrate() {
    if !crate::hpet::available() {
        // no HPET: fall back to the old hard-coded 1 GHz bus assumption
        TICKS_PER_MS = 62_500;
        cal_tag(0);
        return;
    }
    reg(0x3E0).write_volatile(3);                // divide by 16
    reg(0x320).write_volatile(VEC_TIMER as u32); // one-shot, unmasked
    reg(0x380).write_volatile(0xFFFF_FFFF);      // max initial count
    let c0 = reg(0x390).read_volatile() as u64;
    let h0 = crate::hpet::millis();
    while crate::hpet::millis() - h0 < 30 {
        core::hint::spin_loop();
    }
    let c1 = reg(0x390).read_volatile() as u64;
    reg(0x380).write_volatile(0);                // stop the countdown
    // down-counter: elapsed = (c0 - c1) mod 2^32, in post-divider ticks
    let elapsed = c0.wrapping_sub(c1) & 0xFFFF_FFFF;
    let tpm = elapsed / 30;
    TICKS_PER_MS = if tpm < 1 { 1 } else { tpm };
    cal_tag(TICKS_PER_MS);
}

// debugcon: "C" + 8 hex digits of ticks/ms (0 marks the no-HPET fallback)
// so the boot log records the calibration result
unsafe fn cal_tag(v: u64) {
    let mut buf = [0u8; 10];
    buf[0] = b'C';
    let mut shift = 28i32;
    let mut n = 1;
    while shift >= 0 {
        let d = ((v >> shift) & 0xF) as u8;
        buf[n] = if d < 10 { b'0' + d } else { b'A' + d - 10 };
        n += 1;
        shift -= 4;
    }
    for i in 0..n {
        core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") buf[i], options(nomem, nostack));
    }
}

// Arm the one-shot timer for the next SLICE_MS on THIS core. Must run from
// task context (idle loop, syscall, GUI frame) - interrupt-context MMIO
// writes are dropped by QEMU TCG. APs pick this up through their idle loop
// (idle_loop -> tick_service) right after parking.
pub unsafe fn arm_slice() {
    let tpm = core::ptr::read_volatile(&raw const TICKS_PER_MS);
    if tpm == 0 {
        return;
    }
    reg(0x3E0).write_volatile(3);                // divide by 16
    reg(0x320).write_volatile(VEC_TIMER as u32); // one-shot, unmasked
    // TICR in post-divider ticks; clamp so a very fast APIC bus still fits
    let ticr = (tpm as u64 * SLICE_MS).min(0xFFFF_FFFF) as u32;
    reg(0x380).write_volatile(ticr);
    crate::percpu::set_last_arm_ms(crate::hpet::millis());
}

// Called from task context (GUI main loop, syscalls, idle loops): EOI any
// in-service vector stuck by the TCG EOI quirk (e.g. a resched IPI or the
// last timer fire), then re-arm the one-shot if the slice has elapsed.
// Sweeps are rate-limited to 1 ms of HPET time to bound the MMIO traffic
// from yield-heavy tasks.
pub unsafe fn tick_service() {
    let now = crate::hpet::millis();
    if now == crate::percpu::last_sweep_ms() {
        return;
    }
    crate::percpu::set_last_sweep_ms(now);
    reg(0xB0).write_volatile(0);                 // EOI: clear stuck in-service bits
    if now - crate::percpu::last_arm_ms() >= SLICE_MS {
        arm_slice();
    }
}

pub unsafe fn id() -> u8 {
    (reg(0x20).read_volatile() >> 24) as u8
}

#[allow(dead_code)]
pub unsafe fn eoi() {
    reg(0xB0).write_volatile(0);
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

// Fixed-delivery edge IPI: reschedule request -> vector 0x31.
// IMPORTANT: bit14 (level assert, 0x4000) must stay CLEAR for edge mode -
// QEMU keeps a level-asserted ICP "pending" forever (there is no deassert
// write), which silently blocks subsequent ExtINT delivery on the sender.
pub unsafe fn send_ipi(dst: u8, vector: u8) {
    reg(0x310).write_volatile((dst as u32) << 24);
    reg(0x300).write_volatile(0x0000_0000 | vector as u32);
}
