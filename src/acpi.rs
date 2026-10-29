// ACPI table discovery: RSDP -> RSDT/XSDT -> MADT (APIC), HPET.
// Yields the Local APIC base address, the list of processor APIC IDs and
// the HPET timer block base address.

pub static mut LAPIC_BASE: u64 = 0xFEE00000;
pub static mut APIC_IDS: [u8; 8] = [0; 8];
pub static mut CPU_COUNT: usize = 0;
pub static mut HPET_BASE: u64 = 0;

unsafe fn rd_u8(a: u64) -> u8 { core::ptr::read_volatile(a as *const u8) }
unsafe fn rd_u32(a: u64) -> u32 { core::ptr::read_volatile(a as *const u32) }
unsafe fn rd_u64(a: u64) -> u64 { core::ptr::read_volatile(a as *const u64) }

unsafe fn rsdp_valid(a: u64) -> bool {
    if rd_u64(a) != 0x2052_5450_2044_5352 {
        return false; // "RSD PTR "
    }
    let mut sum: u8 = 0;
    for i in 0..20 {
        sum = sum.wrapping_add(rd_u8(a + i));
    }
    sum == 0
}

unsafe fn sig_at(a: u64, s: &[u8; 4]) -> bool {
    rd_u32(a) == u32::from_le_bytes(*s)
}

// walk RSDT (u32 entries) or XSDT (u64 entries) looking for a table by signature
unsafe fn find_table(table: u64, wide: bool, sig: &[u8; 4]) -> u64 {
    if table == 0 {
        return 0;
    }
    let len = rd_u32(table + 4) as u64;
    if len < 36 || len > 0x10000 {
        return 0;
    }
    let mut off = table + 36;
    let end = table + len;
    while off + 4 <= end {
        let entry = if wide { rd_u64(off) } else { rd_u32(off) as u64 };
        off += if wide { 8 } else { 4 };
        if entry == 0 {
            continue;
        }
        if sig_at(entry, sig) {
            return entry;
        }
    }
    0
}

pub unsafe fn init() {
    // scan the EBDA tail and the BIOS area for the Root System Description Pointer
    let mut rsdp: u64 = 0;
    let mut a: u64 = 0xE0000;
    while a < 0xFFFFF {
        if rsdp_valid(a) {
            rsdp = a;
            break;
        }
        a += 16;
    }
    if rsdp == 0 {
        return;
    }
    let revision = rd_u8(rsdp + 15);
    let (rsdt, xsdt) = if revision >= 2 {
        (rd_u32(rsdp + 16) as u64, rd_u64(rsdp + 24))
    } else {
        (rd_u32(rsdp + 16) as u64, 0)
    };
    let mut madt = find_table(xsdt, true, b"APIC");
    if madt == 0 {
        madt = find_table(rsdt, false, b"APIC");
    }
    if madt == 0 {
        return;
    }
    // HPET (optional): the table's GAS field points at the MMIO timer block
    let mut hp = find_table(xsdt, true, b"HPET");
    if hp == 0 {
        hp = find_table(rsdt, false, b"HPET");
    }
    if hp != 0 {
        let len = rd_u32(hp + 4) as u64;
        if len >= 52 {
            // GAS layout: space id @40, bit width @41, bit offset @42,
            // access size @43, 64-bit address @44 (12-byte structure)
            let space = rd_u8(hp + 40); // GAS address space id: 0 = system memory
            let addr = rd_u64(hp + 44);
            if space == 0 && addr != 0 {
                HPET_BASE = addr;
            }
        }
    }
    let lapic_base = rd_u32(madt + 36) as u64;
    if lapic_base != 0 {
        LAPIC_BASE = lapic_base;
    }
    let len = rd_u32(madt + 4) as u64;
    let mut off = madt + 44;
    let end = madt + len;
    let mut count = 0usize;
    while off + 8 <= end {
        let etype = rd_u8(off);
        let elen = rd_u8(off + 1) as u64;
        if elen == 0 {
            break;
        }
        // type 0 = Processor Local APIC
        if etype == 0 {
            let apic_id = rd_u8(off + 3);
            let flags = rd_u32(off + 4);
            if flags & 1 != 0 && count < 8 {
                APIC_IDS[count] = apic_id;
                count += 1;
            }
        }
        off += elen;
    }
    CPU_COUNT = count;
}

// BSP APIC id from IA32_APIC_BASE MSR (bits 24-31)
pub unsafe fn bsp_apic_id() -> u8 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") 0x1Bu32,
        out("eax") lo,
        out("edx") hi,
        options(nomem, nostack)
    );
    ((hi as u64) << 32 | lo as u64) as u8
}
