; MiLuna SMP work-stealing demo (ELF64): pure CPU-bound task, no yields.
; Four copies are spawned by the kernel; idle CPUs steal them from the
; global ready pool on their LAPIC timer ticks, so all four run genuinely
; in parallel (the `tasks` command shows which CPU owns each one).
; emit: rdi=ptr, rsi=len, r15=number to append, r13=1 append number / 0 skip

BITS 64
default rel

%define SYS_WRITE 1
%define SYS_EXIT  3
%define SYS_GETPID 7

section .text
global _start
_start:
    mov rax, SYS_GETPID
    syscall
    mov r15, rax
    lea rdi, [m_pid]
    mov rsi, m_pid_len
    mov r13, 1
    call emit

    mov r14, 0                  ; tick counter (survives syscalls)
.loop:
    ; busy compute phase: the task stays runnable and gets preempted by the
    ; per-CPU LAPIC timer at 50 Hz
    mov rcx, 2000000
.burn:
    dec rcx
    jnz .burn
    inc r14
    cmp r14, 20
    jge .done
    ; print "smp-burnN tick M" every 5 ticks
    mov rax, r14
    xor edx, edx
    mov esi, 5
    div rsi
    test rdx, rdx
    jnz .loop
    mov r15, r14
    lea rdi, [m_tick]
    mov rsi, m_tick_len
    mov r13, 1
    call emit
    jmp .loop
.done:
    mov rax, SYS_EXIT
    xor edi, edi
    syscall

; print text (rdi/rsi), then decimal r15 if r13 != 0, then newline
emit:
    lea r9, [buf]
    mov rcx, rsi
    mov r10, rdi
.c1:
    mov al, [r10]
    mov [r9], al
    inc r10
    inc r9
    dec rcx
    jnz .c1
    test r13, r13
    jz .nonum
    mov rax, r15
    xor rcx, rcx
    mov r11, 10
.div:
    xor edx, edx
    div r11
    add dl, '0'
    push rdx
    inc rcx
    test rax, rax
    jnz .div
.dg:
    pop rax
    mov [r9], al
    inc r9
    dec rcx
    jnz .dg
.nonum:
    mov byte [r9], 10
    inc r9
    lea rax, [buf]
    mov rsi, r9
    sub rsi, rax
    lea rdi, [buf]
    mov rax, SYS_WRITE
    syscall
    ret

section .rodata
m_pid:  db "smp-burn pid="
m_pid_len equ $ - m_pid
m_tick: db "smp-burn tick "
m_tick_len equ $ - m_tick

section .bss
buf: resb 64
