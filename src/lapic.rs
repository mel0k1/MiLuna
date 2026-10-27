// Local APIC: MMIO registers, enable, EOI and INIT/SIPI IPIs for SMP bringup.

pub unsafe fn reg(off: u32) -> *mut u32 {
    (crate::acpi::LAPIC_BASE + off as u64) as *mut u32
}

pub unsafe fn enable() {
    // spurious interrupt register: vector 0xFF + APIC software enable (bit 8)
    reg(0xF0).write_volatile(0x1FF);
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
