; MiLuna copy-on-write fork demo (ELF64).
; Both sides write the SAME .data variable after fork(). With COW the parent
; and child hold private copies: the child sees 7, the parent sees 100.
; A broken COW (shared writable frame) would make the last writer win and
; both would print the same number; a broken fault handler would kill the
; task with a #PF exit code. The printed value is read back from memory.
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
    mov qword [var], 7          ; child's private write -> COW fault + clone
    mov rbx, 0
.cl1:
    mov rax, SYS_YIELD
    syscall
    inc rbx
    cmp rbx, 12
    jl .cl1
    mov r15, [var]              ; read back what the child actually sees
    lea rdi, [m_child]
    mov rsi, m_child_len
    mov r13, 1
    call emit
.cl2:
    mov rax, SYS_YIELD
    syscall
    inc rbx
    cmp rbx, 120
    jl .cl2
    mov rax, SYS_EXIT
    xor edi, edi
    syscall

.parent:
    mov qword [var], 100        ; parent's private write -> own COW fault
    mov rbx, 0
.pl1:
    mov rax, SYS_YIELD
    syscall
    inc rbx
    cmp rbx, 12
    jl .pl1
    mov r15, [var]              ; read back what the parent actually sees
    lea rdi, [m_parent]
    mov rsi, m_parent_len
    mov r13, 1
    call emit
.pl2:
    mov rax, SYS_YIELD
    syscall
    inc rbx
    cmp rbx, 220
    jl .pl2
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
m_pid:    db "cow-demo pid="
m_pid_len equ $ - m_pid
m_child:  db "cow-child sees var="
m_child_len equ $ - m_child
m_parent: db "cow-parent sees var="
m_parent_len equ $ - m_parent

section .data
var: dq 5

section .bss
buf: resb 64
