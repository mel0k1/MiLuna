// Unified trap entry: full context save, task switching, restore.
// Frame layout (192 bytes) must match TrapFrame below.
// Stubs push: [vec] with bit 8 set when slot1 holds an error code.

use core::arch::naked_asm;

#[repr(C)]
pub struct TrapFrame {
    pub rax: u64,
    pub rcx: u64,
    pub rdx: u64,
    pub rbx: u64,
    pub rbp: u64,
    pub rsi: u64,
    pub rdi: u64,
    pub r8: u64,
    pub r9: u64,
    pub r10: u64,
    pub r11: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
    pub pad: u64,
    pub vector: u64,
    pub slot1: u64,
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss: u64,
    pub pad2: u64,
}

#[unsafe(naked)]
pub extern "x86-interrupt" fn timer_entry(_sf: &crate::idt::InterruptStackFrame, _ec: u64) {
    naked_asm!(
        "sub rsp, 8",
        "push 0x20",
        "jmp {c}",
        c = sym trap_common,
    );
}

#[unsafe(naked)]
pub extern "x86-interrupt" fn syscall_entry(_sf: &crate::idt::InterruptStackFrame, _ec: u64) {
    naked_asm!(
        "sub rsp, 8",
        "push 0x80",
        "jmp {c}",
        c = sym trap_common,
    );
}

#[unsafe(naked)]
pub extern "C" fn trap_common() {
    naked_asm!(
        // on stack: vec@0, slot1@8, rip@16, cs@24, rf@32, rsp@40, ss@48
        "push rax",
        "mov ax, [rsp + 56]",
        "cmp ax, 0x10",
        "je 2f",
        // ---- from ring 3: build frame on current task kernel stack ----
        "mov rax, [rip + {cur_task}]",
        "mov rax, [rax]",
        "sub rax, 192",
        "mov [rax + 8], rcx",
        "mov [rax + 16], rdx",
        "mov [rax + 24], rbx",
        "mov [rax + 32], rbp",
        "mov [rax + 40], rsi",
        "mov [rax + 48], rdi",
        "mov [rax + 56], r8",
        "mov [rax + 64], r9",
        "mov [rax + 72], r10",
        "mov [rax + 80], r11",
        "mov [rax + 88], r12",
        "mov [rax + 96], r13",
        "mov [rax + 104], r14",
        "mov [rax + 112], r15",
        "lea rsi, [rip + {rsp0}]",
        "lea rdi, [rax + 128]",
        "lea rsi, [rsi + 8136]",
        "mov rcx, 7",
        "cld",
        "rep movsq",
        "lea rcx, [rip + {rsp0}]",
        "mov rcx, [rcx + 8128]",
        "mov [rax], rcx",
        "mov rsp, rax",
        "jmp 3f",
        // ---- from ring 0: build frame on current kernel stack ----
        "2:",
        "mov rax, rsp",
        "sub rax, 192",
        "and rax, -16",
        "mov [rax + 8], rcx",
        "mov [rax + 16], rdx",
        "mov [rax + 24], rbx",
        "mov [rax + 32], rbp",
        "mov [rax + 40], rsi",
        "mov [rax + 48], rdi",
        "mov [rax + 56], r8",
        "mov [rax + 64], r9",
        "mov [rax + 72], r10",
        "mov [rax + 80], r11",
        "mov [rax + 88], r12",
        "mov [rax + 96], r13",
        "mov [rax + 104], r14",
        "mov [rax + 112], r15",
        "lea rsi, [rsp + 8]",
        "lea rdi, [rax + 128]",
        "mov rcx, 7",
        "cld",
        "rep movsq",
        "mov rcx, [rsp]",
        "mov [rax], rcx",
        "mov rsp, rax",
        // ---- common: fxsave, Rust handler, fxrstor, restore ----
        "3:",
        "mov rax, [rip + {cur_task}]",
        "lea rdi, [rax + 16]",
        "fxsave [rdi]",
        "mov rdi, rsp",
        "call {handle}",
        "test rax, rax",
        "jz 4f",
        "mov rsp, rax",
        "4:",
        "mov rax, [rip + {cur_task}]",
        "lea rdi, [rax + 16]",
        "fxrstor [rdi]",
        "mov rcx, [rsp + 8]",
        "mov rdx, [rsp + 16]",
        "mov rbx, [rsp + 24]",
        "mov rbp, [rsp + 32]",
        "mov rsi, [rsp + 40]",
        "mov rdi, [rsp + 48]",
        "mov r8, [rsp + 56]",
        "mov r9, [rsp + 64]",
        "mov r10, [rsp + 72]",
        "mov r11, [rsp + 80]",
        "mov r12, [rsp + 88]",
        "mov r13, [rsp + 96]",
        "mov r14, [rsp + 104]",
        "mov r15, [rsp + 112]",
        "mov rax, [rsp]",
        "add rsp, 144",
        "iretq",
        cur_task = sym crate::task::CUR_TASK,
        rsp0 = sym crate::gdt::RSP0_STACK,
        handle = sym handle_trap,
    );
}

pub extern "C" fn handle_trap(f: *mut TrapFrame) -> u64 {
    let f = unsafe { &mut *f };
    let has_err = f.vector & 0x100 != 0;
    let vec = f.vector & 0xFF;
    let err = if has_err { f.slot1 } else { 0 };
    let from_user = f.cs & 0x3 == 3;
    match vec {
        0x20 => {
            crate::pit::tick();
            unsafe {
                (*crate::task::CUR_TASK).ticks += 1;
            }
            crate::task::schedule(f)
        }
        0x80 => crate::task::handle_syscall(f),
        0x00..=0x1F => {
            if from_user {
                crate::task::kill_fault(f, vec, err)
            } else {
                crate::idt::kernel_panic(vec, err, f.rip)
            }
        }
        _ => 0,
    }
}
