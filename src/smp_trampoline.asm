; MiLuna SMP trampoline - copied to 0x8000, entered via SIPI vector 0x08.
; Real mode -> PAE/long mode using the kernel's identity page tables,
; then loads the kernel GDT + the per-AP TSS and jumps to smp_ap_entry.
; Patch fields live at offset 0x100 (written by smp.rs before each SIPI).

BITS 16
org 0x8000

start:
    cli
    xor ax, ax
    mov ds, ax
    mov ss, ax
    mov sp, 0x7C00

    mov eax, cr4
    or eax, 0x20               ; PAE
    mov cr4, eax

    mov eax, 0x10000           ; kernel PML4 (identity map, shared)
    mov cr3, eax

    mov ecx, 0xC0000080        ; EFER: LME | SCE | NXE
    rdmsr
    or eax, 0x901
    wrmsr

    mov eax, cr0
    or eax, 0x80000001         ; PG | PE (AP starts with PE=0 after INIT!)
    mov cr0, eax

    lgdt [gdtr]
    jmp 0x08:long_mode

BITS 64
long_mode:
    mov ax, 0x10
    mov ds, ax
    mov es, ax
    mov ss, ax
    mov fs, ax
    mov gs, ax

    lgdt [kgdt]                ; kernel GDT (limit+base patched)
    mov ax, [ktss_sel]         ; this AP's TSS selector (patched)
    ltr ax
    mov rsp, [kstack]          ; private AP stack (patched)
    xor ebp, ebp
    mov rdi, [kapic]           ; apic id argument (patched)
    mov rax, [kentry]          ; smp_ap_entry (patched)
    call rax

.halt:
    cli
    hlt
    jmp .halt

align 8
gdtr:
    dw gdt_end - gdt - 1
    dq gdt
gdt:
    dq 0
    dq 0x00209A000000FFFF      ; 0x08 64-bit code
    dq 0x00CF92000000FFFF      ; 0x10 data
gdt_end:

times 0x100-($-$$) db 0      ; patch area starts at offset 0x100
kgdt:    dw 0                 ; +0x00 kernel Gdtr limit
         dq 0                 ; +0x02 kernel Gdtr base (unaligned: lgdt reads 10 bytes)
ktss_sel: dw 0                ; +0x0A
times 4 db 0                 ; +0x0C pad
kstack:  dq 0                 ; +0x10
kapic:   dq 0                 ; +0x18
kentry:  dq 0                 ; +0x20
