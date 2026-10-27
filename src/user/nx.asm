; MiLuna NX-bit demo: jumps to code placed in a non-executable (.data) page.
; The kernel maps .data with the NX bit set (EFER.NXE + PTE bit 63),
; so the jump raises #PF (instruction fetch) and the task is killed
; with "fault#14" - kernel and other tasks keep running.

BITS 64
default rel

%define SYS_WRITE 1
%define SYS_EXIT  3

section .text
global _start
_start:
    lea rax, [payload]
    jmp rax
.hang:
    jmp .hang

section .data
; Would exit(0) if it were executable - but this page is NX
payload:
    mov rax, SYS_EXIT
    xor edi, edi
    syscall
.here:
    jmp .here
