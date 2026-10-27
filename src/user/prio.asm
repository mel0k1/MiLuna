; MiLuna priority demo (ELF64): a CPU-bound-ish task that yields every iteration.
; Spawned twice by the kernel with different priorities (prio-hi=9, prio-lo=2):
; while the high-priority instance is READY it monopolizes the CPU (starvation),
; the low-priority one only accumulates ticks after the high one exits.

BITS 64
default rel

%define SYS_WRITE 1
%define SYS_YIELD 2
%define SYS_EXIT  3

section .text
global _start
_start:
    mov r15, rdi            ; fault flag arg (0 here)
    mov rbx, 0
.loop:
    ; build "prio task NNN" in .bss buffer
    lea rdi, [buf]
    lea rsi, [pref]
    mov rcx, pref_len
.cp:
    mov al, [rsi]
    mov [rdi], al
    inc rsi
    inc rdi
    dec rcx
    jnz .cp
    mov rax, rbx
    xor edx, edx
    mov esi, 100
    div rsi
    add al, '0'
    mov [rdi], al
    inc rdi
    mov rax, rdx
    xor edx, edx
    mov esi, 10
    div rsi
    add al, '0'
    mov [rdi], al
    inc rdi
    mov al, dl
    add al, '0'
    mov [rdi], al
    inc rdi
    mov byte [rdi], 10
    inc rdi
    mov rax, SYS_WRITE
    lea rdi, [buf]
    mov rsi, msg_len
    syscall
    mov rcx, 800000
.burn:
    dec rcx
    jnz .burn
    mov rax, SYS_YIELD
    syscall
    inc rbx
    cmp rbx, 200
    jl .loop
    mov rax, SYS_EXIT
    xor edi, edi
    syscall

section .rodata
pref: db "prio task tick "
pref_len equ $ - pref
msg_len equ pref_len + 4

section .bss
buf: resb 32
