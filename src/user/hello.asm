; MiLuna ring 3 demo task
; Flat binary, position-independent (rip-relative), loaded anywhere in user region
; Entry args: rdi = 0 normal mode, 1 fault mode (ud2 on tick 5)
; Syscalls: int 0x80, rax = 1 write(ptr,len) / 2 yield / 3 exit(code)

BITS 64
default rel

%define SYS_WRITE 1
%define SYS_YIELD 2
%define SYS_EXIT  3

_start:
    xor r8, r8
    test rdi, rdi
    jz .loop
    mov r8, 1
.loop:
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
    int 0x80
    test r8, r8
    jz .nofault
    cmp rbx, 5
    jl .nofault
    ud2
.nofault:
    ; busy compute phase so the task accumulates CPU ticks under preemption
    mov rcx, 1000000
.burn:
    dec rcx
    jnz .burn
    mov rax, SYS_YIELD
    int 0x80
    inc rbx
    cmp rbx, 400
    jl .loop
    mov rax, SYS_EXIT
    xor edi, edi
    int 0x80
.hang:
    jmp .hang

pref: db "ring3 task tick "
pref_len equ $ - pref
msg_len equ pref_len + 4

section .bss
buf: resb 32
