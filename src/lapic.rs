// Local APIC: MMIO registers, enable, EOI, INIT/SIPI IPIs for SMP bringup,
// per-core timer preemption and fixed IPIs (0x31 = resched) for load-steal.
//
// QEMU TCG quirk: MMIO writes issued from LAPIC-interrupt context are
// dropped, so a periodic LAPIC timer + EOI-in-handler deadlocks after the
// first tick (ISR/IRR stuck, PPR raised). Workaround: run the timer in
// ONE-SHOT mode and re-arm it from TASK context (tick_service) on a PIT
// clock - task-context MMIO writes land reliably. tick_service is called
// from the GUI main loop and from every syscall (yield loop), so every
// active core gets a steady ~50 Hz preemption stream, and the sweep effect
// of the EOI also unsticks any pending IPI delivery.

pub const VEC_TIMER: u8 = 0x24;   // per-core LAPIC timer -> schedule() (0x21 is IRQ1 keyboard!)
pub const VEC_RESCHED: u8 = 0x31; // inter-core "look at the run queue" IPI

// One-shot period small enough to expire well before the 20 ms re-arm
// interval at any plausible bus frequency (62500 * 16ns = 1 ms).
pub const TICR_FIXED: u32 = 62_500;

static mut LAST_TICKED: u64 = 0;

pub unsafe fn reg(off: u32) -> *mut u32 {
    (crate::acpi::LAPIC_BASE + off as u64) as *mut u32
}

pub unsafe fn enable() {
    // spurious interrupt register: vector 0xFF + APIC software enable (bit 8)
    reg(0xF0).write_volatile(0x1FF);
}

// Arm the one-shot timer on THIS core (vector 0x21). APs repeat this in
// smp_ap_entry with the same constant.
pub unsafe fn timer_init() {
    reg(0x3E0).write_volatile(3);            // divide by 16
    reg(0x320).write_volatile(VEC_TIMER as u32); // one-shot, unmasked
    reg(0x380).write_volatile(TICR_FIXED);
}

// Called from task context (GUI main loop, syscalls): the EOI clears any
// in-service vector stuck by the TCG EOI quirk (e.g. a resched IPI), which
// otherwise raises PPR and blocks this core's ExtINT delivery. Rate-limited
// to once per PIT tick.
pub unsafe fn tick_service() {
    let t = crate::pit::ticks();
    if t == core::ptr::read_volatile(&raw const LAST_TICKED) {
        return;
    }
    LAST_TICKED = t;
    reg(0xB0).write_volatile(0);             // EOI: clear stuck in-service bits
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
