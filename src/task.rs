// Kernel tasks and ring 3 user processes with int 0x80 syscalls

use crate::gdt::{USER_CS, USER_SS};
use crate::trap::TrapFrame;

pub const MAX_TASKS: usize = 8;
pub const KSTACK_SIZE: usize = 16 * 1024;
pub const USER_BASE: usize = 0x3F000000;
pub const USER_SLOT: usize = 2 * 1024 * 1024;
// One 4K page table per user slot, placed below the user region.
// Slot phys pages are 1:1, so the PT just re-describes them with per-page rights.
pub const USER_PT_BASE: usize = 0x3E000000;

// Page flags
const PG_PRESENT: u64 = 0x1;
const PG_WRITE: u64 = 0x2;
const PG_USER: u64 = 0x4;
const PG_NX: u64 = 0x8000_0000_0000_0000;

// ELF64 constants
const ELF_MAGIC: [u8; 4] = [0x7F, b'E', b'L', b'F'];
const PT_LOAD: u32 = 1;

pub const ST_FREE: u8 = 0;
pub const ST_READY: u8 = 1;
pub const ST_RUNNING: u8 = 2;
pub const ST_DEAD: u8 = 3;

pub const SYS_WRITE: u64 = 1;
pub const SYS_YIELD: u64 = 2;
pub const SYS_EXIT: u64 = 3;
pub const SYS_FORK: u64 = 4;
pub const SYS_NICE: u64 = 5;
pub const SYS_EXEC: u64 = 6;
pub const SYS_GETPID: u64 = 7;

// Embedded ELF images exec() can switch the calling task to; registered from lib.rs
pub static mut EXEC_BINS: [&[u8]; 4] = [&[]; 4];
pub static mut EXEC_NAMES: [&[u8]; 4] = [&[]; 4];

pub unsafe fn set_exec_bins(bins: [&'static [u8]; 4], names: [&'static [u8]; 4]) {
    EXEC_BINS = bins;
    EXEC_NAMES = names;
}

#[repr(C, align(16))]
pub struct Task {
    pub kstack_top: u64,
    _pad0: u64,
    pub fpu: [u8; 512],
    pub state: u8,
    pub is_user: bool,
    pub fault_mode: bool,
    pub pid: u32,
    pub slot: usize,
    pub ticks: u64,
    pub saved_rsp: u64,
    pub name: [u8; 16],
    pub msg: [u8; 64],
    pub msg_len: usize,
    pub exit_code: u32,
    pub prio: u32,
}

const TASK_TEMPLATE: Task = Task {
    kstack_top: 0,
    _pad0: 0,
    fpu: [0; 512],
    state: ST_FREE,
    is_user: false,
    fault_mode: false,
    pid: 0,
    slot: 0,
    ticks: 0,
    saved_rsp: 0,
    name: [0; 16],
    msg: [0; 64],
    msg_len: 0,
    exit_code: 0,
    prio: 5,
};

pub static mut TASKS: [Task; MAX_TASKS] = [TASK_TEMPLATE; MAX_TASKS];

#[repr(align(16))]
#[derive(Clone, Copy)]
struct KStack([u8; KSTACK_SIZE]);

static mut KSTACKS: [KStack; MAX_TASKS] = [KStack([0; KSTACK_SIZE]); MAX_TASKS];

#[no_mangle]
pub static mut CUR_TASK: *mut Task = core::ptr::null_mut();

static mut SCHED_READY: bool = false;
static mut NEXT_PID: u32 = 10;
pub static mut OUTPUT_DIRTY: bool = false;

pub unsafe fn init() {
    enable_user_pages();
    let t = &mut TASKS[0];
    t.state = ST_RUNNING;
    t.pid = 1;
    t.slot = 0;
    t.is_user = false;
    t.kstack_top = 0;
    // the desktop kernel task runs at the lowest priority: user tasks always
    // preempt it (old scheduler semantics), priority order applies among users
    t.prio = 1;
    let n = b"miluna-gui";
    t.name[..n.len()].copy_from_slice(n);
    CUR_TASK = t as *mut Task;
    SCHED_READY = true;
}

fn enable_user_pages() {
    // identity-map page tables from boot.asm: PML4@0x10000, PDPT@0x11000, PDT0@0x12000
    // set U/S on PML4[0] and PDPT[0]; per-slot PTs are hooked into PDT0[504..512] at spawn
    unsafe {
        let pml4 = 0x10000 as *mut u64;
        core::ptr::write_volatile(pml4, core::ptr::read_volatile(pml4) | 0x4);
        let pdpt = 0x11000 as *mut u64;
        core::ptr::write_volatile(pdpt, core::ptr::read_volatile(pdpt) | 0x4);
    }
}

pub unsafe fn flush_tlb() {
    let cr3: u64;
    core::arch::asm!("mov {}, cr3", out(reg) cr3, options(nomem, nostack));
    core::arch::asm!("mov cr3, {}", in(reg) cr3, options(nomem, nostack));
}

// Map [voff, voff+len) of the slot's user region with per-page flags.
// Physical pages are the slot's own 1:1 pages.
unsafe fn map_user_range(slot: usize, voff: u64, len: u64, flags: u64) {
    let pt = (USER_PT_BASE + slot * 0x1000) as *mut u64;
    let start = (voff >> 12) as usize;
    let end = (((voff + len) + 0xFFF) >> 12) as usize;
    let phys_base = (USER_BASE + slot * USER_SLOT) as u64;
    for i in start..end.min(512) {
        let pte = pt.add(i);
        let phys = phys_base + (i as u64) * 0x1000;
        // union semantics for pages shared by several segments:
        // writable if any segment wants W, executable if any earlier mapping had X
        let old = core::ptr::read_volatile(pte);
        let mut fl = flags;
        if old & PG_PRESENT != 0 {
            if old & PG_NX == 0 {
                fl &= !PG_NX;
            }
            fl |= old & PG_WRITE;
        }
        core::ptr::write_volatile(pte, phys | fl);
    }
}

fn parse_u16(b: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([b[off], b[off + 1]])
}

fn parse_u32(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

fn parse_u64(b: &[u8], off: usize) -> u64 {
    u64::from_le_bytes([
        b[off], b[off + 1], b[off + 2], b[off + 3],
        b[off + 4], b[off + 5], b[off + 6], b[off + 7],
    ])
}

pub fn schedule(f: &mut TrapFrame) -> u64 {
    unsafe {
        if !SCHED_READY {
            return 0;
        }
        let cur = CUR_TASK;
        (*cur).saved_rsp = f as *mut TrapFrame as u64;
        if (*cur).state == ST_RUNNING {
            (*cur).state = ST_READY;
        }
        // highest priority among ready tasks (0 = none ready)
        let mut best: u32 = 0;
        for i in 0..MAX_TASKS {
            if TASKS[i].state == ST_READY && TASKS[i].prio > best {
                best = TASKS[i].prio;
            }
        }
        if best == 0 {
            if (*cur).state == ST_READY {
                (*cur).state = ST_RUNNING;
                return 0;
            }
            let t0 = &mut TASKS[0] as *mut Task;
            if t0 != cur && (*t0).state == ST_READY {
                (*t0).state = ST_RUNNING;
                CUR_TASK = t0;
                return (*t0).saved_rsp;
            }
            if (*cur).state == ST_READY {
                (*cur).state = ST_RUNNING;
            }
            return 0;
        }
        // round-robin among tasks with the top priority, starting after the current slot
        let cur_slot = (*cur).slot;
        let mut next: *mut Task = core::ptr::null_mut();
        for k in 1..=MAX_TASKS {
            let i = (cur_slot + k) % MAX_TASKS;
            if TASKS[i].state == ST_READY && TASKS[i].prio == best {
                next = &mut TASKS[i] as *mut Task;
                break;
            }
        }
        if next.is_null() {
            if (*cur).state == ST_READY {
                (*cur).state = ST_RUNNING;
            }
            return 0;
        }
        (*next).state = ST_RUNNING;
        CUR_TASK = next;
        (*next).saved_rsp
    }
}

pub fn handle_syscall(f: &mut TrapFrame) -> u64 {
    unsafe {
        let cur = CUR_TASK;
        match f.rax {
            SYS_WRITE => {
                let mut n = 0usize;
                let len = (f.rsi as usize).min(48);
                let src = f.rdi as *const u8;
                while n < len {
                    let b = core::ptr::read_volatile(src.add(n));
                    if b == 0 || b == 0x0A {
                        break;
                    }
                    (*cur).msg[n] = b;
                    n += 1;
                }
                (*cur).msg_len = n;
                OUTPUT_DIRTY = true;
                f.rax = 0;
            }
            SYS_YIELD => {
                return schedule(f);
            }
            SYS_EXIT => {
                (*cur).exit_code = f.rdi as u32;
                (*cur).state = ST_DEAD;
                OUTPUT_DIRTY = true;
                return schedule(f);
            }
            SYS_FORK => {
                let cslot = (*cur).slot;
                let mut slot = usize::MAX;
                for i in 1..MAX_TASKS {
                    if i != cslot && (TASKS[i].state == ST_FREE || TASKS[i].state == ST_DEAD) {
                        slot = i;
                        break;
                    }
                }
                if slot == usize::MAX {
                    f.rax = u64::MAX;
                    return 0;
                }
                // build the child's PT first (copy of the parent's, retargeted to the
                // child's own pages) and hook it in - the 2MB copy below would fault
                // on a not-yet-mapped slot otherwise
                let pt_src = (USER_PT_BASE + cslot * 0x1000) as *const u64;
                let pt_dst = (USER_PT_BASE + slot * 0x1000) as *mut u64;
                core::ptr::write_bytes(pt_dst as *mut u8, 0, 0x1000);
                for i in 0..512 {
                    let e = core::ptr::read_volatile(pt_src.add(i));
                    if e & PG_PRESENT == 0 {
                        continue;
                    }
                    let idx = ((e >> 12) & 0x1FF) as usize;
                    let phys = (USER_BASE + slot * USER_SLOT + idx * 0x1000) as u64;
                    core::ptr::write_volatile(pt_dst.add(i), phys | (e & (0xFFF | PG_NX)));
                }
                let pdt = 0x12000 as *mut u64;
                core::ptr::write_volatile(
                    pdt.add(504 + slot),
                    ((USER_PT_BASE + slot * 0x1000) as u64) | PG_PRESENT | PG_WRITE | PG_USER,
                );
                flush_tlb();
                // duplicate only the pages that actually exist in the parent's
                // address space - unmapped holes between segments must not fault
                let src_base = (USER_BASE + cslot * USER_SLOT) as u64;
                let dst_base = (USER_BASE + slot * USER_SLOT) as u64;
                for i in 0..512 {
                    let e = core::ptr::read_volatile(pt_src.add(i));
                    if e & PG_PRESENT == 0 {
                        continue;
                    }
                    core::ptr::copy_nonoverlapping(
                        (src_base + (i as u64) * 0x1000) as *const u8,
                        (dst_base + (i as u64) * 0x1000) as *mut u8,
                        0x1000,
                    );
                }
                let t = &mut TASKS[slot] as *mut Task;
                core::ptr::copy_nonoverlapping((*cur).fpu.as_ptr(), (*t).fpu.as_mut_ptr(), 512);
                (*t).kstack_top = (KSTACKS[slot].0.as_ptr() as usize + KSTACK_SIZE) as u64;
                (*t).slot = slot;
                (*t).is_user = true;
                (*t).fault_mode = (*cur).fault_mode;
                (*t).ticks = 0;
                (*t).msg_len = 0;
                (*t).exit_code = 0;
                (*t).prio = (*cur).prio;
                (*t).pid = NEXT_PID;
                NEXT_PID += 1;
                (*t).name = (*cur).name;
                // child frame: copy of the parent's; the syscall entry stored the
                // RETURN address in rip (past the syscall instruction) already,
                // so the child resumes right after fork() with rax = 0.
                // rip and rsp are parent-slot linear addresses - rebase them to the
                // child's slot, otherwise the child would run the parent's image
                // through the shared address space until that slot gets reused
                let pbase = (USER_BASE + cslot * USER_SLOT) as u64;
                let cbase = (USER_BASE + slot * USER_SLOT) as u64;
                let fp = ((*t).kstack_top - 192) as *mut TrapFrame;
                core::ptr::copy_nonoverlapping(f as *const TrapFrame, fp, 1);
                (*fp).rax = 0;
                (*fp).rip = cbase + ((*fp).rip - pbase);
                (*fp).rsp = cbase + ((*fp).rsp - pbase);
                (*t).saved_rsp = fp as u64;
                (*t).state = ST_READY;
                OUTPUT_DIRTY = true;
                f.rax = (*t).pid as u64;
            }
            SYS_NICE => {
                (*cur).prio = (f.rdi as u32).clamp(1, 9);
                f.rax = (*cur).prio as u64;
            }
            SYS_GETPID => {
                f.rax = (*cur).pid as u64;
            }
            SYS_EXEC => {
                let idx = f.rdi as usize;
                let bins = core::ptr::addr_of!(EXEC_BINS) as *const &[u8];
                let names = core::ptr::addr_of!(EXEC_NAMES) as *const &[u8];
                if idx >= 4 || (*bins.add(idx)).is_empty() {
                    f.rax = u64::MAX;
                    return 0;
                }
                let bin: &[u8] = *bins.add(idx);
                let name: &[u8] = *names.add(idx);
                let slot = (*cur).slot;
                if !elf_valid(bin) || load_elf(slot, bin) < 0 {
                    f.rax = u64::MAX;
                    return 0;
                }
                let base = USER_BASE + slot * USER_SLOT;
                (*cur).msg_len = 0;
                for j in 0..16 {
                    (*cur).name[j] = if j < name.len() { name[j] } else { 0 };
                }
                // brand-new frame at the same kernel stack position: the old user
                // image is gone, execution continues inside the new one
                let fp = ((*cur).kstack_top - 192) as *mut TrapFrame;
                core::ptr::write_bytes(fp as *mut u8, 0, 192);
                (*fp).vector = 0;
                (*fp).slot1 = 0;
                (*fp).rip = base as u64 + parse_u64(bin, 0x18);
                (*fp).cs = USER_CS as u64;
                (*fp).rflags = 0x202;
                (*fp).rsp = (base + USER_SLOT - 64) as u64;
                (*fp).ss = USER_SS as u64;
                (*cur).saved_rsp = fp as u64;
                OUTPUT_DIRTY = true;
                return fp as u64;
            }
            _ => {
                f.rax = u64::MAX;
            }
        }
        0
    }
}

pub fn kill_fault(f: &mut TrapFrame, vec: u64, err: u64) -> u64 {
    unsafe {
        let cur = CUR_TASK;
        let cr2 = if vec == 14 {
            let v: u64;
            core::arch::asm!("mov {}, cr2", out(reg) v, options(nomem, nostack));
            v
        } else {
            0
        };
        let mut n = 0usize;
        let mut put = |b: u8| {
            if n < 63 {
                (*cur).msg[n] = b;
                n += 1;
            }
        };
        let prefix = b"fault#";
        for &b in prefix {
            put(b);
        }
        put_dec(&mut put, vec);
        put(b' ');
        let prefix2 = b"err=";
        for &b in prefix2 {
            put(b);
        }
        put_dec(&mut put, err);
        if vec == 14 {
            let p3 = b" cr2=";
            for &b in p3 {
                put(b);
            }
            put_dec(&mut put, cr2);
        }
        (*cur).msg_len = n;
        (*cur).exit_code = (256 + vec) as u32;
        (*cur).state = ST_DEAD;
        OUTPUT_DIRTY = true;
    }
    schedule(f)
}

fn put_dec(put: &mut impl FnMut(u8), mut v: u64) {
    let mut digits = [0u8; 20];
    let mut i = 0;
    if v == 0 {
        put(b'0');
        return;
    }
    while v > 0 {
        digits[i] = b'0' + (v % 10) as u8;
        v /= 10;
        i += 1;
    }
    while i > 0 {
        i -= 1;
        put(digits[i]);
    }
}

fn with_irqs_disabled<R>(f: impl FnOnce() -> R) -> R {
    let flags: u64;
    unsafe {
        core::arch::asm!(
            "pushfq",
            "pop {fl}",
            "cli",
            fl = out(reg) flags,
            options(nomem, nostack)
        );
    }
    let r = f();
    unsafe {
        core::arch::asm!("push {fl}", "popfq", fl = in(reg) flags, options(nomem, nostack));
    }
    r
}

pub fn spawn_elf(bin: &[u8], name: &[u8], fault: bool, prio: u32) -> i32 {
    with_irqs_disabled(|| unsafe {
        if !elf_valid(bin) {
            return -2;
        }
        let mut slot = usize::MAX;
        for i in 1..MAX_TASKS {
            if TASKS[i].state == ST_FREE || TASKS[i].state == ST_DEAD {
                slot = i;
                break;
            }
        }
        if slot == usize::MAX {
            return -1;
        }
        if load_elf(slot, bin) < 0 {
            return -3;
        }
        let base = USER_BASE + slot * USER_SLOT;
        let entry = base as u64 + parse_u64(bin, 0x18);
        let t = &mut TASKS[slot] as *mut Task;
        core::ptr::write_bytes((*t).fpu.as_mut_ptr(), 0, 512);
        *((*t).fpu.as_mut_ptr() as *mut u16) = 0x037F;
        *((*t).fpu.as_mut_ptr().add(24) as *mut u32) = 0x1F80;
        (*t).kstack_top = (KSTACKS[slot].0.as_ptr() as usize + KSTACK_SIZE) as u64;
        (*t).slot = slot;
        (*t).is_user = true;
        (*t).fault_mode = fault;
        (*t).ticks = 0;
        (*t).msg_len = 0;
        (*t).exit_code = 0;
        (*t).pid = NEXT_PID;
        NEXT_PID += 1;
        (*t).prio = prio.clamp(1, 9);
        for j in 0..16 {
            (*t).name[j] = if j < name.len() { name[j] } else { 0 };
        }
        let fp = ((*t).kstack_top - 192) as *mut TrapFrame;
        let fr = &mut *fp;
        core::ptr::write_bytes(fr as *mut TrapFrame as *mut u8, 0, 192);
        fr.rdi = if fault { 1 } else { 0 };
        fr.vector = 0;
        fr.slot1 = 0;
        fr.rip = entry;
        fr.cs = USER_CS as u64;
        fr.rflags = 0x202;
        fr.rsp = (base + USER_SLOT - 64) as u64;
        fr.ss = USER_SS as u64;
        (*t).saved_rsp = fp as u64;
        (*t).state = ST_READY;
        (*t).pid as i32
    })
}

fn elf_valid(bin: &[u8]) -> bool {
    if bin.len() < 64 || bin[0..4] != ELF_MAGIC || bin[4] != 2 || bin[5] != 1 {
        return false;
    }
    // x86-64
    if parse_u16(bin, 0x12) != 0x3E {
        return false;
    }
    let phoff = parse_u64(bin, 0x20);
    let phentsize = parse_u16(bin, 0x36) as u64;
    let phnum = parse_u16(bin, 0x38) as u64;
    phoff > 0 && phoff + phnum * phentsize <= bin.len() as u64 && phnum > 0 && phnum <= 32
}

// Parse ELF64 program headers, copy segments into the slot and map them
// with per-page rights: text R+X, rodata R, data/stack RW, all non-X pages NX.
unsafe fn load_elf(slot: usize, bin: &[u8]) -> i32 {
    let e_entry = parse_u64(bin, 0x18);
    let phoff = parse_u64(bin, 0x20) as usize;
    let phentsize = parse_u16(bin, 0x36) as usize;
    let phnum = parse_u16(bin, 0x38) as usize;
    if e_entry >= (USER_SLOT - 64 * 1024) as u64 {
        return -1;
    }
    let pt = (USER_PT_BASE + slot * 0x1000) as *mut u64;
    core::ptr::write_bytes(pt as *mut u8, 0, 0x1000);
    // hook slot PT into PDT0[504+slot]: present | write | user
    let pdt = 0x12000 as *mut u64;
    core::ptr::write_volatile(pdt.add(504 + slot), (pt as u64) | PG_PRESENT | PG_WRITE | PG_USER);
    // stack: last 64KB of slot, RW + NX (code/segments are limited below it)
    let stack_off = (USER_SLOT - 64 * 1024) as u64;
    map_user_range(slot, stack_off, 64 * 1024, PG_PRESENT | PG_WRITE | PG_USER | PG_NX);
    let base = USER_BASE + slot * USER_SLOT;
    for i in 0..phnum {
        let p = &bin[phoff + i * phentsize..];
        if parse_u32(p, 0) != PT_LOAD {
            continue;
        }
        let flags = parse_u32(p, 4);
        let off = parse_u64(p, 8) as usize;
        let vaddr = parse_u64(p, 16);
        let filesz = parse_u64(p, 32);
        let memsz = parse_u64(p, 40);
        if vaddr >= stack_off || memsz > stack_off || vaddr + memsz > stack_off {
            return -1;
        }
        if off + filesz as usize > bin.len() {
            return -1;
        }
        // map BEFORE copying: fresh PT pages are not-present, the segment copy
        // would fault on them otherwise; CR0.WP=0 lets ring 0 write R+X pages
        let mut fl = PG_PRESENT | PG_USER;
        if flags & 2 != 0 {
            fl |= PG_WRITE;
        }
        if flags & 1 == 0 {
            fl |= PG_NX;
        }
        map_user_range(slot, vaddr, memsz, fl);
        core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
        let dst = (base as u64 + vaddr) as *mut u8;
        if filesz > 0 {
            core::ptr::copy_nonoverlapping(bin[off..].as_ptr(), dst, filesz as usize);
        }
        if memsz > filesz {
            core::ptr::write_bytes(dst.add(filesz as usize), 0, (memsz - filesz) as usize);
        }
    }
    // fresh page-table state: stale TLB entries from a previous task in this slot
    // (possibly NX) would fault the kernel's own segment-copy or future accesses
    flush_tlb();
    0
}

#[derive(Clone, Copy)]
pub struct TaskInfo {
    pub pid: u32,
    pub name: [u8; 16],
    pub state: u8,
    pub ticks: u64,
    pub msg: [u8; 64],
    pub msg_len: usize,
    pub exit_code: u32,
}

pub fn snapshot(i: usize) -> Option<TaskInfo> {
    unsafe {
        if i >= MAX_TASKS {
            return None;
        }
        let t = &TASKS[i];
        if t.state == ST_FREE {
            return None;
        }
        Some(TaskInfo {
            pid: t.pid,
            name: t.name,
            state: t.state,
            ticks: t.ticks,
            msg: t.msg,
            msg_len: t.msg_len,
            exit_code: t.exit_code,
        })
    }
}

pub fn state_name(s: u8) -> &'static str {
    match s {
        ST_RUNNING => "Running",
        ST_READY => "Ready",
        ST_DEAD => "Dead",
        _ => "Free",
    }
}

pub fn stats() -> (usize, usize, u64, u64) {
    let mut count = 0;
    let mut user_count = 0;
    let mut total = 0u64;
    let mut user = 0u64;
    unsafe {
        for i in 0..MAX_TASKS {
            let t = &TASKS[i];
            if t.state == ST_FREE {
                continue;
            }
            count += 1;
            total += t.ticks;
            if t.is_user {
                user_count += 1;
                user += t.ticks;
            }
        }
    }
    (count, user_count, total, user)
}

pub fn take_dirty() -> bool {
    unsafe {
        let d = OUTPUT_DIRTY;
        OUTPUT_DIRTY = false;
        d
    }
}

pub fn list_tasks() -> &'static str {
    static mut BUF: [u8; 1600] = [0; 1600];
    static mut LEN: usize = 0;
    unsafe {
        let buf = core::ptr::addr_of_mut!(BUF);
        let len = core::ptr::addr_of_mut!(LEN);
        *len = 0;
        let mut put = |b: u8| {
            if *len < 1590 {
                (*buf)[*len] = b;
                *len += 1;
            }
        };
        let mut put_pad = |s: &[u8], w: usize| {
            let mut n = 0;
            for &c in s {
                put(c);
                n += 1;
            }
            while n < w {
                put(b' ');
                n += 1;
            }
        };
        put_pad(b"PID", 5);
        put_pad(b"NAME", 12);
        put_pad(b"STATE", 9);
        put_pad(b"PRIO", 5);
        put_pad(b"TICKS", 7);
        put_pad(b"MESSAGE", 20);
        put(b'\n');
        for i in 0..MAX_TASKS {
            let t = &TASKS[i];
            if t.state == ST_FREE {
                continue;
            }
            let mut digs = [0u8; 10];
            let mut dn = 0;
            let mut p = t.pid;
            if p == 0 {
                digs[0] = b'0';
                dn = 1;
            }
            while p > 0 {
                digs[dn] = b'0' + (p % 10) as u8;
                p /= 10;
                dn += 1;
            }
            let mut num = [0u8; 10];
            for k in 0..dn {
                num[k] = digs[dn - 1 - k];
            }
            put_pad(&num[..dn], 5);
            let mut nend = 0;
            while nend < 16 && t.name[nend] != 0 {
                nend += 1;
            }
            put_pad(&t.name[..nend], 12);
            put_pad(state_name(t.state).as_bytes(), 9);
            let mut pv = t.prio as u64;
            let mut pd = [0u8; 4];
            let mut pn = 0;
            if pv == 0 {
                pd[0] = b'0';
                pn = 1;
            }
            while pv > 0 && pn < 4 {
                pd[pn] = b'0' + (pv % 10) as u8;
                pv /= 10;
                pn += 1;
            }
            let mut pr = [0u8; 4];
            for k in 0..pn {
                pr[k] = pd[pn - 1 - k];
            }
            put_pad(&pr[..pn], 5);
            let mut tv = t.ticks;
            let mut td = [0u8; 20];
            let mut tn = 0;
            if tv == 0 {
                td[0] = b'0';
                tn = 1;
            }
            while tv > 0 {
                td[tn] = b'0' + (tv % 10) as u8;
                tv /= 10;
                tn += 1;
            }
            let mut num2 = [0u8; 20];
            for k in 0..tn {
                num2[k] = td[tn - 1 - k];
            }
            put_pad(&num2[..tn], 7);
            if t.state == ST_DEAD {
                let mut code = [0u8; 12];
                let cl = {
                    let cv = t.exit_code as u64;
                    let mut n = 0;
                    let mut v = cv;
                    if v == 0 {
                        code[0] = b'0';
                        n = 1;
                    }
                    while v > 0 {
                        code[n] = b'0' + (v % 10) as u8;
                        v /= 10;
                        n += 1;
                    }
                    let mut rev = [0u8; 12];
                    for k in 0..n {
                        rev[k] = code[n - 1 - k];
                    }
                    code = rev;
                    n
                };
                put_pad(b"exit ", 5);
                put_pad(&code[..cl], 40);
            } else {
                let mlen = t.msg_len.min(64);
                let mut m = [0u8; 64];
                for k in 0..mlen {
                    m[k] = if t.msg[k] == 0 { b' ' } else { t.msg[k] };
                }
                put_pad(&m[..mlen], 40);
            }
            put(b'\n');
        }
        core::str::from_utf8_unchecked(core::slice::from_raw_parts(
            core::ptr::addr_of!(BUF) as *const u8,
            *len,
        ))
    }
}
