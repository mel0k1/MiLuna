; MiLuna HPET-sleep demo (ELF64).
; Sleeps 3 x 400ms through SYS_SLEEP (rax=10, rdi=ms): the task parks in
; ST_SLEEPING with an HPET deadline and is woken by wake_sleepers() at the
; next scheduling event after the deadline - watch the Sleeping state in
; `tasks` while it runs. Between sleeps it reports its pid.
; emit: rdi=ptr, rsi=len, r15=number to append, r13=1 append number / 0 skip

BITS 64
default rel

%define SYS_WRITE  1
%define SYS_EXIT   3
%define SYS_GETPID 7
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

    mov rbx, 1
.round:
    mov r15, rbx
    lea rdi, [m_go]
    mov rsi, m_go_len
    mov r13, 1
    call emit

    mov rdi, 400                ; 400 ms on the HPET clock
    mov rax, SYS_SLEEP
    syscall

    mov r15, rbx
    lea rdi, [m_up]
    mov rsi, m_up_len
    mov r13, 1
    call emit
    inc rbx
    cmp rbx, 4
    jl .round

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
    sub rsi, rax                ; total length
    lea rdi, [buf]
    mov rax, SYS_WRITE
    syscall
    ret

section .rodata
m_pid:  db "sleep-demo pid="
m_pid_len equ $ - m_pid
m_go:   db "sleep round "
m_go_len equ $ - m_go
m_up:   db "woke from HPET-sleep (round "
m_up_len equ $ - m_up
m_done: db "sleep-demo: done - slept 3 x 400ms"
m_done_len equ $ - m_done

section .bss
buf: resb 64
