/* MiLuna user program linker script: static ELF64, base 0x10000 (fits 2MB slot).
   Sections are 4K-aligned so each lands on its own page with distinct rights:
   text R+X, rodata R, data/bss RW, everything except text carries the NX bit. */
ENTRY(_start)
SECTIONS {
  . = 0x10000;
  .text : { *(.text*) }
  . = ALIGN(4K);
  .rodata : { *(.rodata*) }
  . = ALIGN(4K);
  .data : { *(.data*) }
  . = ALIGN(4K);
  .bss : { *(.bss*) *(COMMON) }
}
