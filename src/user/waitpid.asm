; MiLuna waitpid-with-PID demo (ELF64).
; Parent forks TWO children: child-1 sleeps 300ms then exits 111, child-2
; sleeps 700ms then exits 222. The parent blocks in waitpid(child-2 PID)
; FIRST - the kernel must NOT wake it when child-1 dies earlier (make_zombie
; only matches wait_pid == 0 or the exact pid). Only when child-2 exits does
; the parked frame get (222<<32)|pid2. A second waitpid(any) reaps child-1.
; Uses HPET-sleep (SYS_SLEEP) for the child delays.
; emit: rdi=ptr, rsi=len, r15=number to append, r13=1 append number / 0 skip

BITS 64
default rel

%define SYS_WRITE  1
%define SYS_YIELD  2
%define SYS_EXIT   3
%define SYS_FORK   4
%define SYS_GETPID 7
%define SYS_WAIT   9
%define SYS_SLEEP  10

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
    jz .child1
    mov r14, rax                ; parent: pid1

    mov rax, SYS_FORK
    syscall
    test rax, rax
    jz .child2
    mov rbx, rax                ; parent: pid2 (emit clobbers rax/r15)

; ---- parent ----
    mov r13, 0
    lea rdi, [m_two]
    mov rsi, m_two_len
    call emit
    mov r15, r14
    lea rdi, [m_c1]
    mov rsi, m_c1_len
    mov r13, 1
    call emit
.parent_wait:
    ; waitpid(child-2): rdi = pid2 - must block past child-1's 300ms exit
    lea rdi, [m_w2]
    mov rsi, m_w2_len
    mov r13, 0
    call emit
    mov rdi, rbx                ; specific PID
    mov rax, SYS_WAIT
    syscall                     ; rax = (222 << 32) | pid2
    mov r14, rax                ; save result
    mov r15, rax
    shr r15, 32                 ; exit code -> must be 222
    lea rdi, [m_got2]
    mov rsi, m_got2_len
    mov r13, 1
    call emit
    mov r15d, r14d              ; low 32 bits: pid -> must equal pid2
    lea rdi, [m_gotp]
    mov rsi, m_gotp_len
    mov r13, 1
    call emit
    ; waitpid(any): reaps the already-zombie child-1 -> exit 111
    lea rdi, [m_wa]
    mov rsi, m_wa_len
    mov r13, 0
    call emit
    xor edi, edi
    mov rax, SYS_WAIT
    syscall
    mov r15, rax
    shr r15, 32                 ; -> 111
    lea rdi, [m_got1]
    mov rsi, m_got1_len
    mov r13, 1
    call emit
    ; hold the result visible in `tasks`: short yield cycles
    ; (a busy hold would starve the desktop task)
    mov r12, 0
.hold:
    mov rcx, 2000000
.h2:
    dec rcx
    jnz .h2
    mov rax, SYS_YIELD
    syscall
    inc r12
    cmp r12, 80
    jl .hold
    mov rax, SYS_EXIT
    xor edi, edi
    syscall

; ---- child 1: 300ms HPET-sleep, exit 111 ----
.child1:
    mov rax, SYS_GETPID
    syscall
    mov r15, rax
    lea rdi, [m_c1go]
    mov rsi, m_c1go_len
    mov r13, 1
    call emit
    mov rdi, 300
    mov rax, SYS_SLEEP
    syscall
    mov r15, 111
    lea rdi, [m_c1ex]
    mov rsi, m_c1ex_len
    mov r13, 1
    call emit
    mov rax, SYS_EXIT
    mov edi, 111
    syscall

; ---- child 2: 700ms HPET-sleep, exit 222 ----
.child2:
    mov rax, SYS_GETPID
    syscall
    mov r15, rax
    lea rdi, [m_c2go]
    mov rsi, m_c2go_len
    mov r13, 1
    call emit
    mov rdi, 700
    mov rax, SYS_SLEEP
    syscall
    mov r15, 222
    lea rdi, [m_c2ex]
    mov rsi, m_c2ex_len
    mov r13, 1
    call emit
    mov rax, SYS_EXIT
    mov edi, 222
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
    sub rsi, rax                ; total length
    lea rdi, [buf]
    mov rax, SYS_WRITE
    syscall
    ret

section .rodata
m_pid:   db "waitpid-demo pid="
m_pid_len equ $ - m_pid
m_two:   db "parent: forked two children"
m_two_len equ $ - m_two
m_c1:    db "parent: child-1 pid="
m_c1_len equ $ - m_c1
m_w2:    db "parent: blocking in waitpid(child-2)..."
m_w2_len equ $ - m_w2
m_got2:  db "parent: waitpid(child-2) -> exit "
m_got2_len equ $ - m_got2
m_gotp:  db "parent: reaped pid "
m_gotp_len equ $ - m_gotp
m_wa:    db "parent: waitpid(any) -> exit "
m_wa_len equ $ - m_wa
m_got1:  db "parent: waitpid(any) code "
m_got1_len equ $ - m_got1
m_c1go:  db "child-1 pid="
m_c1go_len equ $ - m_c1go
m_c1ex:  db "child-1: slept 300ms, exit "
m_c1ex_len equ $ - m_c1ex
m_c2go:  db "child-2 pid="
m_c2go_len equ $ - m_c2go
m_c2ex:  db "child-2: slept 700ms, exit "
m_c2ex_len equ $ - m_c2ex

section .bss
buf: resb 64
