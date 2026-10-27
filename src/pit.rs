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
