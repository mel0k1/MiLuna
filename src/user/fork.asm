; MiLuna fork demo (ELF64): prints its PID, forks, parent reports the child PID,
; child prints its own PID and exits 0. Fork returns 0 in the child, child PID
; in the parent (rax) - the classic UNIX contract.
; emit: rdi=ptr, rsi=len, r15=number to append, r13=1 append number / 0 skip

BITS 64
default rel

%define SYS_WRITE 1
%define SYS_YIELD 2
%define SYS_EXIT  3
%define SYS_FORK  4
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

    mov rax, SYS_FORK
    syscall
    test rax, rax
    jnz .parent

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
    mov rcx, 400000
.b1:
    dec rcx
    jnz .b1
    mov rax, SYS_YIELD
    syscall
    inc rbx
    cmp rbx, 20
    jl .cl
    mov rax, SYS_EXIT
    xor edi, edi
    syscall

.parent:
    mov r14, rax            ; child pid
    lea rdi, [m_pchild]
    mov rsi, m_pchild_len
    mov r13, 1
    call emit
    mov rbx, 0
.pl:
    mov rcx, 400000
.p1:
    dec rcx
    jnz .p1
    mov rax, SYS_YIELD
    syscall
    inc rbx
    cmp rbx, 60
    jl .pl
    lea rdi, [m_done]
    mov rsi, m_done_len
    mov r13, 0
    call emit
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
m_pid:    db "fork-demo pid="
m_pid_len equ $ - m_pid
m_child:  db "fork-child pid="
m_child_len equ $ - m_child
m_pchild: db "fork-parent: child pid="
m_pchild_len equ $ - m_pchild
m_done:   db "fork-parent: done"
m_done_len equ $ - m_done

section .bss
buf: resb 64
