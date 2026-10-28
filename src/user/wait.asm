; MiLuna wait/exit demo (ELF64). The parent blocks in wait() (new ST_WAITING
; state, the scheduler skips it) until the child exits with code 42; the
; kernel wakes the parent and hands the exit code back through the parked
; syscall frame, so wait() returns 42 in rax. rdi = 0 waits for any child.
; emit: rdi=ptr, rsi=len, r15=number to append, r13=1 append number / 0 skip

BITS 64
default rel

%define SYS_WRITE 1
%define SYS_YIELD 2
%define SYS_EXIT  3
%define SYS_FORK  4
%define SYS_GETPID 7
%define SYS_WAIT  8

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
    lea rdi, [m_child]
    mov rsi, m_child_len
    mov r13, 0
    call emit
    mov rbx, 0
.cl:
    mov rax, SYS_YIELD
    syscall
    inc rbx
    cmp rbx, 25
    jl .cl
    mov rax, SYS_EXIT
    mov edi, 42                 ; the code the parent must receive
    syscall

.parent:
    lea rdi, [m_waiting]
    mov rsi, m_waiting_len
    mov r13, 0
    call emit
    mov rax, SYS_WAIT
    xor edi, edi                ; wait(any child) - blocks this task
    syscall                     ; returns the child's exit code in rax
    mov r14, rax
    mov r15, r14
    lea rdi, [m_code]
    mov rsi, m_code_len
    mov r13, 1
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
    sub rsi, rax
    lea rdi, [buf]
    mov rax, SYS_WRITE
    syscall
    ret

section .rodata
m_pid:     db "wait-demo pid="
m_pid_len equ $ - m_pid
m_child:   db "wait-child: exiting with code 42"
m_child_len equ $ - m_child
m_waiting: db "wait-parent: blocking in wait()..."
m_waiting_len equ $ - m_waiting
m_code:    db "wait-parent: child exit code="
m_code_len equ $ - m_code

section .bss
buf: resb 64
