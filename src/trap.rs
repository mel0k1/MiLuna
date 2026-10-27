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

// Fast syscall entry (SYSCALL instruction, MSR-programmed via gdt::init_syscall).
// On entry: ring 0 on the USER stack, rcx = user rip, r11 = user rflags,
// rax = syscall number, interrupts already off (SFMASK cleared IF).
// Builds the same 192-byte TrapFrame the int 0x80 path ends up with, on the
// task's own kernel stack, then dispatches into handle_trap. Returns to ring 3
// via SYSRET - unless the scheduler switched to the kernel task, whose frames
// must return via IRETQ.
#[unsafe(naked)]
pub extern "C" fn syscall_fast_entry() {
    naked_asm!(
        // save user rsp, then build the frame on the CURRENT TASK'S kernel stack
        // (kstack_top-192) - never in the RSP0 area: hardware traps push their iret
        // frame at the RSP0 top, which would corrupt a syscall frame parked there
        // while the task runs in user mode. r10 holds the Task* (its frame slot
        // gets the scratch value instead of the user's r10 - caller-clobbered ABI;
        // r8/r9/r11 stay untouched apart from the hardware rcx/r11 clobber).
        "mov [rip + {scratch}], rsp",
        "mov r10, [rip + {cur_task}]",
        "mov rsp, [r10]",
        "sub rsp, 192",
        "add rsp, 128",
        // 16 pushes land rax at frame+0; rcx/r11 slots get informational values
        "push 0",
        "push r15",
        "push r14",
        "push r13",
        "push r12",
        "push r11",
        "push r10",
        "push r9",
        "push r8",
        "push rdi",
        "push rsi",
        "push rbp",
        "push rbx",
        "push rdx",
        "push rcx",
        "push rax",
        // iret portion of the frame
        "mov qword ptr [rsp + 128], 0x80",
        "mov qword ptr [rsp + 136], 0",
        "mov [rsp + 144], rcx",
        "mov qword ptr [rsp + 152], {cs3}",
        "mov [rsp + 160], r11",
        "mov rax, [rip + {scratch}]",
        "mov [rsp + 168], rax",
        "mov qword ptr [rsp + 176], {ss3}",
        // fpu state (Task+16), Rust handler
        "lea rdi, [r10 + 16]",
        "fxsave [rdi]",
        "mov rdi, rsp",
        "call {handle}",
        "test rax, rax",
        "jz 2f",
        "mov rsp, rax",
        "2:",
        "mov r10, [rip + {cur_task}]",
        "lea rdi, [r10 + 16]",
        "fxrstor [rdi]",
        // ring3 target -> sysret; ring0 target (kernel task) -> iretq
        "mov rcx, [rsp + 152]",
        "test cl, 3",
        "jz 4f",
        "mov rax, [rsp]",
        "mov rdx, [rsp + 16]",
        "mov rbx, [rsp + 24]",
        "mov rbp, [rsp + 32]",
        "mov rsi, [rsp + 40]",
        "mov rdi, [rsp + 48]",
        "mov r8, [rsp + 56]",
        "mov r9, [rsp + 64]",
        "mov r10, [rsp + 72]",
        "mov r12, [rsp + 88]",
        "mov r13, [rsp + 96]",
        "mov r14, [rsp + 104]",
        "mov r15, [rsp + 112]",
        "mov rcx, [rsp + 144]",
        "mov r11, [rsp + 160]",
        "mov rsp, [rsp + 168]",
        "sysretq",
        "4:",
        "mov rcx, [rsp + 8]",
        "mov r11, [rsp + 80]",
        "mov rax, [rsp]",
        "mov rdx, [rsp + 16]",
        "mov rbx, [rsp + 24]",
        "mov rbp, [rsp + 32]",
        "mov rsi, [rsp + 40]",
        "mov rdi, [rsp + 48]",
        "mov r8, [rsp + 56]",
        "mov r9, [rsp + 64]",
        "mov r10, [rsp + 72]",
        "mov r12, [rsp + 88]",
        "mov r13, [rsp + 96]",
        "mov r14, [rsp + 104]",
        "mov r15, [rsp + 112]",
        "add rsp, 144",
        "iretq",
        scratch = sym SYSCALL_SCRATCH,
        cs3 = const 0x23u64,
        ss3 = const 0x1Bu64,
        cur_task = sym crate::task::CUR_TASK,
        handle = sym handle_trap,
    );
}

static mut SYSCALL_SCRATCH: u64 = 0;

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
        // guard: frame.cs must be kernel or user code selector before iretq
        "mov rcx, [rsp + 8]",
        "cmp cx, 0x08",
        "je 9f",
        "cmp cx, 0x23",
        "je 9f",
        "mov rsi, rsp",
        "sub rsi, 144",
        "mov rbx, 24",
        "8:",
        "mov rax, [rsi]",
        "mov rcx, 16",
        "7:",
        "rol rax, 4",
        "mov rdx, rax",
        "and dl, 0x0f",
        "cmp dl, 10",
        "jb 6f",
        "add dl, 39",
        "6:",
        "add dl, 48",
        "push rax",
        "mov rax, rdx",
        "mov rdx, 0xe9",
        "out dx, al",
        "pop rax",
        "dec rcx",
        "jnz 7b",
        "mov al, 32",
        "mov rdx, 0xe9",
        "out dx, al",
        "add rsi, 8",
        "dec rbx",
        "jnz 8b",
        "mov al, 84",
        "mov rdx, 0xe9",
        "out dx, al",
        "mov al, 10",
        "out dx, al",
        "cli",
        "hlt",
        "jmp 8b",
        "9:",
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
