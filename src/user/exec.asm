; MiLuna exec demo (ELF64): prints a message, then replaces its own image with
; the registered exec target 0 (hello) via SYS_EXEC. The kernel loads the new
; ELF into the same slot, renames the task and restarts it at the new entry.

BITS 64
default rel

%define SYS_WRITE 1
%define SYS_YIELD 2
%define SYS_EXIT  3
%define SYS_EXEC  6

section .text
global _start
_start:
    lea rdi, [msg]
    mov rsi, msg_len
    mov rax, SYS_WRITE
    syscall
    ; brief moment so `tasks` can catch the old name
    mov rcx, 3000000
.wait:
    dec rcx
    jnz .wait
    mov rax, SYS_YIELD
    syscall
    ; exec into image 0 (hello) - SYS_EXEC never returns for the caller
    mov rax, SYS_EXEC
    xor edi, edi
    syscall
    ; if exec failed (rax = -1), fall through to a graceful exit
    mov rax, SYS_EXIT
    mov edi, 1
    syscall

section .rodata
msg: db "exec-demo: replacing image with hello (exec 0)"
msg_len equ $ - msg
