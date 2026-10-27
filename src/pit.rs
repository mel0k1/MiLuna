// Programmable Interval Timer: IRQ0 tick source for preemptive scheduling

static mut TICKS: u64 = 0;

pub const TIMER_HZ: u64 = 50;

fn inb(port: u16) -> u8 {
    let v: u8;
    unsafe { core::arch::asm!("in al, dx", in("dx") port, out("al") v, options(nomem, nostack)) }
    v
}

fn outb(port: u16, val: u8) {
    unsafe { core::arch::asm!("out dx, al", in("dx") port, in("al") val, options(nomem, nostack)) }
}

pub unsafe fn init() {
    let div: u16 = (1193182 / TIMER_HZ) as u16;
    outb(0x43, 0x36);
    outb(0x40, (div & 0xFF) as u8);
    outb(0x40, (div >> 8) as u8);
    let mask = inb(0x21);
    outb(0x21, mask & !0x01);
}

pub fn tick() {
    unsafe {
        TICKS += 1;
        outb(0x20, 0x20);
    }
}

pub fn ticks() -> u64 {
    unsafe { core::ptr::read_volatile(&raw const TICKS) }
}

// retire IRQ0 once the LAPIC timer takes over as the tick source.
// The spurious EOI matters: the PIT fired at least once between pit::init
// and this call, and a stuck in-service IRQ0 on the 8259A blocks every
// lower-priority line - keyboard (IRQ1) and mouse would go dead.
pub fn mask_irq0() {
    unsafe {
        let m = inb(0x21);
        outb(0x21, m | 0x01);
        outb(0x20, 0x20); // non-specific EOI: unstick the in-service IRQ0
    }
}
