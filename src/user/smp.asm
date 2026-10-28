; MiLuna SMP load demo (ELF64): forks 3 CPU-bound workers (COW shares the
; image pages until each worker writes its own counter). With 4 cores the
; scheduler spreads the four runnable tasks; `cpuinfo` shows per-core ticks,
; stolen task counts and the current task, `tasks` shows owner cores.
; emit: rdi=ptr, rsi=len, r15=number to append, r13=1 append number / 0 skip

BITS 64
default rel

%define SYS_WRITE  1
%define SYS_YIELD  2
%define SYS_EXIT   3
%define SYS_FORK   4
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

    xor r12, r12            ; fork loop counter
.forkloop:
    mov rax, SYS_FORK
    syscall
    test rax, rax
    jz .worker
    inc r12
    cmp r12, 3
    jl .forkloop

    ; ---- parent: also burns CPU for ~3.5s, then exits ----
    mov r15, rax            ; last child pid (report once)
    lea rdi, [m_workers]
    mov rsi, m_workers_len
    mov r13, 1
    call emit
    mov rbx, 0
.pl:
    mov rcx, 6000000
.pb:
    dec rcx
    jnz .pb
    mov rax, SYS_YIELD
    syscall
    inc rbx
    cmp rbx, 40
    jl .pl
    lea rdi, [m_done]
    mov rsi, m_done_len
    mov r13, 0
    call emit
    ; hold final message visible: short yield cycles (desktop keeps slices)
    mov rbx, 0
.hold:
    mov rcx, 2000000
.h2:
    dec rcx
    jnz .h2
    mov rax, SYS_YIELD
    syscall
    inc rbx
    cmp rbx, 10
    jl .hold
    mov rax, SYS_EXIT
    xor edi, edi
    syscall

; ---- worker: private counter copy (COW), CPU-bound ticks, exit 0 ----
.worker:
    mov rax, SYS_GETPID
    syscall
    mov r15, rax
    lea rdi, [m_worker]
    mov rsi, m_worker_len
    mov r13, 1
    call emit
    mov rbx, 0
.wl:
    inc qword [counter]     ; COW write fault -> private page
    mov r15, [counter]
    lea rdi, [m_tick]
    mov rsi, m_tick_len
    mov r13, 1
    call emit
    mov rcx, 6000000
.wb:
    dec rcx
    jnz .wb
    mov rax, SYS_YIELD
    syscall
    inc rbx
    cmp rbx, 60
    jl .wl
    ; hold final 'smp tick N' visible in `tasks` (yield cycles)
    mov rbx, 0
.whold:
    mov rcx, 2000000
.wh2:
    dec rcx
    jnz .wh2
    mov rax, SYS_YIELD
    syscall
    inc rbx
    cmp rbx, 10
    jl .whold
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
    sub rsi, rax            ; total length
    lea rdi, [buf]
    mov rax, SYS_WRITE
    syscall
    ret

section .rodata
m_pid:    db "smp-demo pid="
m_pid_len equ $ - m_pid
m_worker: db "smp-worker pid="
m_worker_len equ $ - m_worker
m_tick:   db "smp tick "
m_tick_len equ $ - m_tick
m_workers: db "smp-main: workers forked (last pid="
m_workers_len equ $ - m_workers
m_done:   db "smp-main: done"
m_done_len equ $ - m_done

section .data
counter: dq 0

section .bss
buf: resb 64
