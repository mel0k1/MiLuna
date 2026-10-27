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
// The CPU is resolved with one MMIO read of the LAPIC id register and a
// lookup in task::PER_CPU (stride 32): its scratch field parks the user rsp
// and `cur` provides the kernel stack. Builds the same 192-byte TrapFrame
// the int 0x80 path ends up with, then dispatches into handle_trap. Returns
// to ring 3 via SYSRET - unless the scheduler switched to the kernel task or
// the task exited and the CPU went idle (both return via IRETQ).
#[unsafe(naked)]
pub extern "C" fn syscall_fast_entry() {
    naked_asm!(
        // r10 is the scratch register (caller-clobbered ABI): it briefly holds
        // &PER_CPU[me], then the Task*. rax is saved on the user stack until
        // the frame build - syscalls only come from ring 3.
        "push rax",
        "mov eax, dword ptr [0xFEE00020]",
        "shr rax, 24",
        "imul rax, rax, 32",
        "lea r10, [rip + {per_cpu}]",
        "add rax, r10",
        "lea r10, [rsp + 8]",
        "mov [rax + 16], r10", // scratch = user rsp
        "mov r10, [rax]", // current task
        "mov rax, [rsp]", // restore syscall number
        "add rsp, 8",
        "mov rsp, [r10]", // switch to the task's kernel stack
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
        "mov eax, dword ptr [0xFEE00020]",
        "shr rax, 24",
        "imul rax, rax, 32",
        "lea rdx, [rip + {per_cpu}]",
        "add rax, rdx",
        "mov rdx, [rax + 16]", // saved user rsp
        "mov [rsp + 168], rdx",
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
        // re-resolve the current task: handle_trap may have switched tasks on
        // this CPU - or gone idle (cur = 0) after the task exited
        "mov eax, dword ptr [0xFEE00020]",
        "shr rax, 24",
        "imul rax, rax, 32",
        "lea rdx, [rip + {per_cpu}]",
        "add rax, rdx",
        "mov r10, [rax]",
        "test r10, r10",
        "jz 3f",
        "lea rdi, [r10 + 16]",
        "fxrstor [rdi]",
        "3:",
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
        cs3 = const 0x23u64,
        ss3 = const 0x1Bu64,
        per_cpu = sym crate::task::PER_CPU,
        handle = sym handle_trap,
    );
}

#[unsafe(naked)]
pub extern "C" fn trap_common() {
    naked_asm!(
        // on stack: vec@0, slot1@8, rip@16, cs@24, rf@32, [rsp@40, ss@48 ring 3]
        // three pushes park user rax/rcx/rdx below the frame while the CPU
        // index is resolved from the LAPIC id register
        "push rax",
        "push rcx",
        "push rdx",
        // layout: rdx@0, rcx@8, rax@16, vec@24, slot1@32, rip@40, cs@48, rf@56,
        //         [rsp@64, ss@72 when from ring 3]
        "mov ax, [rsp + 48]",
        "cmp ax, 0x08", // kernel code segment -> ring 0 origin
        "je 2f",
        // ---- from ring 3: frame on the per-CPU current task's kernel stack ----
        "mov eax, dword ptr [0xFEE00020]",
        "shr rax, 24",
        "imul rax, rax, 32",
        "lea rcx, [rip + {per_cpu}]",
        "add rax, rcx",
        "mov rcx, [rax]", // current task (0 = idle CPU)
        "test rcx, rcx",
        "jz 2f", // idle CPU: park the frame on the current stack
        "mov rdx, [rcx]", // kstack_top
        "sub rdx, 192", // rdx = frame base
        "mov [rdx + 24], rbx",
        "mov [rdx + 32], rbp",
        "mov [rdx + 40], rsi",
        "mov [rdx + 48], rdi",
        "mov [rdx + 56], r8",
        "mov [rdx + 64], r9",
        "mov [rdx + 72], r10",
        "mov [rdx + 80], r11",
        "mov [rdx + 88], r12",
        "mov [rdx + 96], r13",
        "mov [rdx + 104], r14",
        "mov [rdx + 112], r15",
        "mov rbx, [rsp + 16]",
        "mov [rdx], rbx", // user rax
        "mov rbx, [rsp + 8]",
        "mov [rdx + 8], rbx", // user rcx
        "mov rbx, [rsp]",
        "mov [rdx + 16], rbx", // user rdx
        // iret portion (vec..ss) from this CPU's own RSP0 transit area
        "mov rbx, [rax + 8]", // per-cpu rsp0_top
        "lea rsi, [rbx - 56]",
        "lea rdi, [rdx + 128]",
        "mov rcx, 7",
        "cld",
        "rep movsq",
        "mov rsp, rdx",
        "jmp 3f",
        // ---- from ring 0 (or idle CPU): frame on the current kernel stack ----
        "2:",
        "mov rax, rsp",
        "sub rax, 192",
        "and rax, -16",
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
        "mov rcx, [rsp + 16]",
        "mov [rax], rcx", // user rax
        "mov rcx, [rsp + 8]",
        "mov [rax + 8], rcx", // user rcx
        "mov rcx, [rsp]",
        "mov [rax + 16], rcx", // user rdx
        "lea rsi, [rsp + 24]",
        "lea rdi, [rax + 128]",
        "mov rcx, 7",
        "cld",
        "rep movsq",
        "mov rsp, rax",
        // ---- common: fxsave, Rust handler, fxrstor, restore ----
        "3:",
        "mov eax, dword ptr [0xFEE00020]",
        "shr rax, 24",
        "imul rax, rax, 32",
        "lea rbx, [rip + {per_cpu}]",
        "add rax, rbx",
        "mov rbx, [rax]", // current task
        "test rbx, rbx",
        "jz 6f",
        "lea rdi, [rbx + 16]",
        "fxsave [rdi]",
        "6:",
        "mov rdi, rsp",
        "call {handle}",
        "test rax, rax",
        "jz 4f",
        "mov rsp, rax",
        "4:",
        "mov eax, dword ptr [0xFEE00020]",
        "shr rax, 24",
        "imul rax, rax, 32",
        "lea rbx, [rip + {per_cpu}]",
        "add rax, rbx",
        "mov rbx, [rax]",
        "test rbx, rbx",
        "jz 7f",
        "lea rdi, [rbx + 16]",
        "fxrstor [rdi]",
        "7:",
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
        "dmp8:",
        "mov rax, [rsi]",
        "mov rcx, 16",
        "dmp7:",
        "rol rax, 4",
        "mov rdx, rax",
        "and dl, 0x0f",
        "cmp dl, 10",
        "jb dmp6",
        "add dl, 39",
        "dmp6:",
        "add dl, 48",
        "push rax",
        "mov rax, rdx",
        "mov rdx, 0xe9",
        "out dx, al",
        "pop rax",
        "dec rcx",
        "jnz dmp7",
        "mov al, 32",
        "mov rdx, 0xe9",
        "out dx, al",
        "add rsi, 8",
        "dec rbx",
        "jnz dmp8",
        "mov al, 84",
        "mov rdx, 0xe9",
        "out dx, al",
        "mov al, 10",
        "out dx, al",
        "cli",
        "hlt",
        "jmp dmp8",
        "9:",
        "iretq",
        per_cpu = sym crate::task::PER_CPU,
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
            // LAPIC timer tick on this CPU: EOI the local APIC, bump the
            // current task's accounting and run the per-CPU scheduler
            // (which also implements task stealing for idle CPUs)
            unsafe {
                crate::lapic::eoi();
                core::ptr::write_volatile(
                    &raw mut crate::lapic::TICK_ARRIVALS,
                    core::ptr::read_volatile(&raw const crate::lapic::TICK_ARRIVALS).wrapping_add(1),
                );
                let cur = crate::task::cpu_cur();
                if !cur.is_null() {
                    (*cur).ticks += 1;
                    (*cur).beat = crate::timer::millis();
                }
                // BSP housekeeping: re-arm tasks whose owner CPU lost them
                if crate::lapic::id() == crate::task::bsp_apic() {
                    crate::task::wedged_tasks_reset();
                }
            }
            crate::task::schedule(f)
        }
        0x80 => crate::task::handle_syscall(f),
        0x00..=0x1F => {
            if from_user {
                // write access to a COW-shared page: clone it and re-execute
                if vec == 14 && crate::task::cow_fix(f) {
                    return 0;
                }
                crate::task::kill_fault(f, vec, err)
            } else {
                crate::idt::kernel_panic(vec, err, f.rip)
            }
        }
        _ => 0,
    }
}
