use core::arch::naked_asm;
use core::mem::size_of;

const IDT_ENTRIES: usize = 256;

#[repr(C)]
pub struct InterruptStackFrame {
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss: u64,
}

#[repr(C, packed)]
#[derive(Copy, Clone)]
struct IdtEntry {
    offset_low: u16,
    selector: u16,
    ist: u8,
    flags: u8,
    offset_mid: u16,
    offset_high: u32,
    reserved: u32,
}

impl IdtEntry {
    const fn new() -> Self {
        Self {
            offset_low: 0,
            selector: 0,
            ist: 0,
            flags: 0,
            offset_mid: 0,
            offset_high: 0,
            reserved: 0,
        }
    }

    pub fn set_handler_addr(&mut self, handler_addr: u64) {
        self.offset_low = (handler_addr & 0xFFFF) as u16;
        self.selector = 0x08;
        self.ist = 0;
        self.flags = 0x8E;
        self.offset_mid = ((handler_addr >> 16) & 0xFFFF) as u16;
        self.offset_high = (handler_addr >> 32) as u32;
        self.reserved = 0;
    }
}

#[repr(C, packed)]
struct IdtPtr {
    limit: u16,
    base: u64,
}

static mut IDT: [IdtEntry; IDT_ENTRIES] = [IdtEntry::new(); IDT_ENTRIES];


macro_rules! gen_stub {
    ($name:ident, $num:expr) => {
        #[unsafe(naked)]
        extern "x86-interrupt" fn $name(_sf: &InterruptStackFrame) {
            naked_asm!(
                "sub rsp, 8",
                "push {num}",
                "jmp {c}",
                num = const $num,
                c = sym crate::trap::trap_common
            );
        }
    };
}


macro_rules! gen_stub_error {
    ($name:ident, $num:expr) => {
        #[unsafe(naked)]
        extern "x86-interrupt" fn $name(_sf: &InterruptStackFrame, _error_code: u64) {
            naked_asm!(
                "push {num}",
                "jmp {c}",
                num = const ($num | 0x100),
                c = sym crate::trap::trap_common
            );
        }
    };
}

gen_stub!(isr0, 0);
gen_stub!(isr1, 1);
gen_stub!(isr2, 2);
gen_stub!(isr3, 3);
gen_stub!(isr4, 4);
gen_stub!(isr5, 5);
gen_stub!(isr6, 6);
gen_stub!(isr7, 7);
gen_stub!(isr9, 9);
gen_stub!(isr15, 15);
gen_stub!(isr16, 16);
gen_stub!(isr18, 18);
gen_stub!(isr19, 19);
gen_stub!(isr20, 20);
gen_stub!(isr21, 21);
gen_stub!(isr22, 22);
gen_stub!(isr23, 23);
gen_stub!(isr24, 24);
gen_stub!(isr25, 25);
gen_stub!(isr26, 26);
gen_stub!(isr27, 27);
gen_stub!(isr28, 28);
gen_stub!(isr29, 29);
gen_stub!(isr30, 30);
gen_stub!(isr31, 31);

gen_stub_error!(isr8, 8);
gen_stub_error!(isr10, 10);
gen_stub_error!(isr11, 11);
gen_stub_error!(isr12, 12);
gen_stub_error!(isr13, 13);
gen_stub_error!(isr14, 14);
gen_stub_error!(isr17, 17);

const EXC_NAMES: [&str; 32] = [
    "Divide Error", "Debug", "NMI", "Breakpoint", "Overflow", "BOUND Range",
    "Invalid Opcode", "Device NA", "Double Fault", "Coproc Seg", "Invalid TSS",
    "Segment Not Present", "Stack Fault", "General Protection", "Page Fault",
    "Reserved", "x87 FPU", "Alignment Check", "Machine Check", "SIMD FP",
    "Virt Exception", "Reserved", "Reserved", "Reserved", "Reserved",
    "Reserved", "Reserved", "Reserved", "Reserved", "Reserved", "Security", "Reserved",
];

fn panic_put_px(x: usize, y: usize, c: u32) {
    unsafe {
        let (addr, stride, w, h) = crate::graphics::FB_INFO;
        if addr == 0 || x >= w || y >= h { return; }
        let p = (addr as *mut u32).add(y * stride + x);
        core::ptr::write_volatile(p, c);
    }
}

fn panic_fill(x: usize, y: usize, w: usize, h: usize, c: u32) {
    for row in 0..h { for col in 0..w { panic_put_px(x + col, y + row, c); } }
}

fn panic_char(x: usize, y: usize, ch: u8, c: u32) {
    if ch < 32 || ch > 126 { return; }
    let base = (ch as usize) * 16;
    for row in 0..16 {
        let byte = crate::tamzen_font::FONT[base + row];
        for col in 0..8 {
            if byte & (1 << (7 - col)) != 0 { panic_put_px(x + col, y + row, c); }
        }
    }
}

fn panic_text(x: usize, y: usize, s: &str, c: u32) {
    let mut cx = x;
    for b in s.bytes() {
        panic_char(cx, y, b, c);
        cx += 8;
        if cx + 8 > 1920 { break; }
    }
}

fn panic_hex(x: usize, y: usize, v: u64, c: u32) {
    const HEX: [u8; 16] = *b"0123456789ABCDEF";
    let mut buf = [0u8; 16];
    let mut n = 0usize;
    if v == 0 { buf[0] = b'0'; n = 1; }
    else {
        let mut v = v;
        while v > 0 && n < 16 { buf[n] = HEX[(v & 0xF) as usize]; v >>= 4; n += 1; }
    }
    let mut cx = x;
    for i in (0..n).rev() { panic_char(cx, y, buf[i], c); cx += 8; }
}

fn vga_print(s: &[u8], row: u16, col: u16, attr: u8) {
    let mut c = col;
    for &b in s {
        let off = (row * 80 + c) * 2;
        unsafe {
            core::ptr::write_volatile((0xB8000 + off as usize) as *mut u8, b);
            core::ptr::write_volatile((0xB8000 + off as usize + 1) as *mut u8, attr);
        }
        c += 1;
    }
}

pub fn kernel_panic(vec: u64, err: u64, rip: u64) -> ! {
    let name = if (vec as usize) < 32 { EXC_NAMES[vec as usize] } else { "Unknown" };
    unsafe {
        let (addr, _, w, h) = crate::graphics::FB_INFO;
        if addr != 0 {
            let bg = 0xA80000u32;
            let fg = 0xFFFFFFu32;
            panic_fill(0, 0, w, h, bg);
            panic_fill(0, 0, w, 64, 0x600000);
            panic_text(24, 20, "MiLuna kernel panic", 0xFFD0D0);
            panic_text(24, 100, "Exception:", fg);
            panic_text(136, 100, name, fg);
            panic_text(24, 128, "Vector:", fg);
            panic_hex(104, 128, vec, fg);
            panic_text(24, 156, "Error code:", fg);
            panic_hex(152, 156, err, fg);
            panic_text(24, 184, "RIP:", fg);
            panic_hex(72, 184, rip, fg);
            if vec == 14 {
                let cr2: u64;
                core::arch::asm!("mov {}, cr2", out(reg) cr2);
                panic_text(24, 212, "CR2:", fg);
                panic_hex(72, 212, cr2, fg);
            }
            panic_text(24, 280, "The system has been halted.", fg);
            panic_text(24, 300, "Please report: github.com/mel0k1/MiLuna", 0xC0C0C0);
        } else {
            vga_print(b"MiLuna kernel panic", 0, 0, 0x1F);
            vga_print(name.as_bytes(), 2, 0, 0x0F);
        }
    }
    loop { unsafe { core::arch::asm!("cli", "hlt"); } }
}

pub unsafe fn set_handler(vector: usize, handler: extern "x86-interrupt" fn(&InterruptStackFrame, u64)) {
    if vector < IDT_ENTRIES {
        IDT[vector].set_handler_addr(handler as u64);
    }
}

pub unsafe fn set_gate_flags(vector: usize, handler_addr: u64, flags: u8) {
    if vector < IDT_ENTRIES {
        IDT[vector].set_handler_addr(handler_addr);
        IDT[vector].flags = flags;
    }
}

pub unsafe fn init_idt() {
    IDT[0].set_handler_addr(isr0 as u64);
    IDT[1].set_handler_addr(isr1 as u64);
    IDT[2].set_handler_addr(isr2 as u64);
    IDT[3].set_handler_addr(isr3 as u64);
    IDT[4].set_handler_addr(isr4 as u64);
    IDT[5].set_handler_addr(isr5 as u64);
    IDT[6].set_handler_addr(isr6 as u64);
    IDT[7].set_handler_addr(isr7 as u64);
    IDT[8].set_handler_addr(isr8 as u64);
    IDT[9].set_handler_addr(isr9 as u64);
    IDT[10].set_handler_addr(isr10 as u64);
    IDT[11].set_handler_addr(isr11 as u64);
    IDT[12].set_handler_addr(isr12 as u64);
    IDT[13].set_handler_addr(isr13 as u64);
    IDT[14].set_handler_addr(isr14 as u64);
    IDT[15].set_handler_addr(isr15 as u64);
    IDT[16].set_handler_addr(isr16 as u64);
    IDT[17].set_handler_addr(isr17 as u64);
    IDT[18].set_handler_addr(isr18 as u64);
    IDT[19].set_handler_addr(isr19 as u64);
    IDT[20].set_handler_addr(isr20 as u64);
    IDT[21].set_handler_addr(isr21 as u64);
    IDT[22].set_handler_addr(isr22 as u64);
    IDT[23].set_handler_addr(isr23 as u64);
    IDT[24].set_handler_addr(isr24 as u64);
    IDT[25].set_handler_addr(isr25 as u64);
    IDT[26].set_handler_addr(isr26 as u64);
    IDT[27].set_handler_addr(isr27 as u64);
    IDT[28].set_handler_addr(isr28 as u64);
    IDT[29].set_handler_addr(isr29 as u64);
    IDT[30].set_handler_addr(isr30 as u64);
    IDT[31].set_handler_addr(isr31 as u64);

    IDT[0x20].set_handler_addr(crate::trap::timer_entry as u64);
    IDT[0x24].set_handler_addr(crate::trap::lapic_timer_entry as u64);
    IDT[0x31].set_handler_addr(crate::trap::resched_entry as u64);
    IDT[0x80].set_handler_addr(crate::trap::syscall_entry as u64);
    IDT[0x80].flags = 0xEE;

    let idt_ptr = IdtPtr {
        limit: (size_of::<[IdtEntry; IDT_ENTRIES]>() - 1) as u16,
        base: IDT.as_ptr() as u64,
    };

    core::arch::asm!("lidt [{}]", in(reg) &idt_ptr);
}

// APs must load the shared IDT themselves: IDTR is per-CPU. Same table.
pub unsafe fn reload() {
    let idt_ptr = IdtPtr {
        limit: (size_of::<[IdtEntry; IDT_ENTRIES]>() - 1) as u16,
        base: IDT.as_ptr() as u64,
    };
    core::arch::asm!("lidt [{}]", in(reg) &idt_ptr);
}
