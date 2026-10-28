; MiLuna reap-race stress (ELF64). Forks children that exit after a tiny
; pseudo-random delay (0..3 yields), so child exits, waiter wake-ups and
; reaps land on top of the OTHER cores' fork/exit paths. Two instances of
; this demo double the churn. Parent waits all children and reports codes.
; emit: rdi=ptr, rsi=len, r15=number to append, r13=1 append number / 0 skip

BITS 64
default rel

%define SYS_WRITE  1
%define SYS_YIELD  2
%define SYS_EXIT   3
%define SYS_FORK   4
%define SYS_GETPID 7
%define SYS_WAIT   9

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

    xor r12, r12
.forkloop:
    mov rax, SYS_FORK
    syscall
    test rax, rax
    jz .child
    inc r12
    cmp r12, 6
    jl .forkloop

    ; ---- parent: wait for all 6 children, report each code ----
    xor r12, r12
.wloop:
    mov rax, SYS_WAIT
    xor edi, edi
    syscall                 ; rax = (code<<32)|pid, or -1
    cmp rax, -1
    je .wdone
    mov r14, rax            ; keep result
    shr r14, 32             ; code
    mov r15, r14
    lea rdi, [m_code]
    mov rsi, m_code_len
    mov r13, 1
    call emit
    inc r12
    cmp r12, 12             ; safety bound: never spin forever
    jl .wloop
.wdone:
    lea rdi, [m_done]
    mov rsi, m_done_len
    mov r13, 0
    call emit
    mov rax, SYS_EXIT
    xor edi, edi
    syscall

; ---- child: burn pid&3 yields, exit 7 ----
.child:
    mov rax, SYS_GETPID
    syscall
    and rax, 3              ; tiny pseudo-random burn 0..3 yields
    mov rbx, rax
.cl:
    test rbx, rbx
    jz .cdone
    mov rax, SYS_YIELD
    syscall
    dec rbx
    jnz .cl
.cdone:
    mov rax, SYS_EXIT
    mov edi, 7
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
m_pid:    db "race pid="
m_pid_len equ $ - m_pid
m_code:   db "race: child exit code="
m_code_len equ $ - m_code
m_done:   db "race: done"
m_done_len equ $ - m_done

section .bss
buf: resb 64
