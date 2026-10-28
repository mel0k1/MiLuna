; MiLuna signals + waitpid + COW demo (ELF64):
; parent forks a child (COW: both share pages R/O), the child writes its own
; copy of `counter` (write fault -> private page), the parent sends SIGTERM,
; then blocks in waitpid and prints the exit code (128+15 = 143).
; The parent prints its own counter (still 0) as COW evidence.
; emit: rdi=ptr, rsi=len, r15=number to append, r13=1 append number / 0 skip

BITS 64
default rel

%define SYS_WRITE  1
%define SYS_YIELD  2
%define SYS_EXIT   3
%define SYS_FORK   4
%define SYS_GETPID 7
%define SYS_KILL   8
%define SYS_WAIT   9
%define SIGTERM    15

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

    mov rax, SYS_FORK
    syscall
    test rax, rax
    jnz .parent

; ---- child: writes its private counter copy, gets killed by SIGTERM ----
.child:
    mov rax, SYS_GETPID
    syscall
    mov r15, rax
    lea rdi, [m_child]
    mov rsi, m_child_len
    mov r13, 1
    call emit
    mov rbx, 0
.cl:
    inc qword [counter]     ; COW write fault: private copy of the page
    mov r15, [counter]
    lea rdi, [m_tick]
    mov rsi, m_tick_len
    mov r13, 1
    call emit
    mov rcx, 60000000
.b1:
    dec rcx
    jnz .b1
    mov rax, SYS_YIELD
    syscall
    inc rbx
    cmp rbx, 30
    jl .cl
    mov rax, SYS_EXIT       ; never reached: SIGTERM lands first
    xor edi, edi
    syscall

; ---- parent: kill(SIGTERM), waitpid, report exit code + COW evidence ----
.parent:
    mov r14, rax            ; child pid
    mov r15, rax
    lea rdi, [m_pchild]
    mov rsi, m_pchild_len
    mov r13, 1
    call emit
    mov rbx, 0
.y1:
    mov rcx, 60000000
.yb:
    dec rcx
    jnz .yb
    mov rax, SYS_YIELD
    syscall
    inc rbx
    cmp rbx, 10
    jl .y1
    ; COW evidence: the child incremented its copy, the parent's is untouched
    mov r15, [counter]
    lea rdi, [m_cow]
    mov rsi, m_cow_len
    mov r13, 1
    call emit
    ; kill(child, SIGTERM)
    mov rdi, r14
    mov rsi, SIGTERM
    mov rax, SYS_KILL
    syscall
    lea rdi, [m_kill]
    mov rsi, m_kill_len
    mov r13, 0
    call emit
    ; waitpid(child) -> rax = (exit_code << 32) | pid
    mov rdi, r14
    mov rax, SYS_WAIT
    syscall
    mov r15, rax
    shr r15, 32             ; exit code (143 = 128 + SIGTERM)
    lea rdi, [m_wait]
    mov rsi, m_wait_len
    mov r13, 1
    call emit
    ; hold the result visible in `tasks`: short yield cycles
    ; (a busy hold would starve the desktop task)
    mov rbx, 0
.hold:
    mov rcx, 2000000
.h2:
    dec rcx
    jnz .h2
    mov rax, SYS_YIELD
    syscall
    inc rbx
    cmp rbx, 80
    jl .hold
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
m_pid:    db "sig-demo pid="
m_pid_len equ $ - m_pid
m_child:  db "sig-child pid="
m_child_len equ $ - m_child
m_tick:   db "sig-child tick "
m_tick_len equ $ - m_tick
m_pchild: db "sig-parent: child pid="
m_pchild_len equ $ - m_pchild
m_cow:    db "sig-parent: my counter= "
m_cow_len equ $ - m_cow
m_kill:   db "sig-parent: sent SIGTERM, waiting..."
m_kill_len equ $ - m_kill
m_wait:   db "sig: waitpid -> exit "
m_wait_len equ $ - m_wait

section .data
counter: dq 0

section .bss
buf: resb 64
