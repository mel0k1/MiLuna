// Kernel tasks and ring 3 user processes with syscall/sysret.
// Phase D: per-process page tables (own PML4/CR3 per task), COW fork with a
// refcounted frame pool, SMP load-steal scheduling and signals/waitpid.

use crate::gdt::{USER_CS, USER_SS};
use crate::trap::TrapFrame;

pub const MAX_TASKS: usize = 8;
pub const KSTACK_SIZE: usize = 16 * 1024;
// Per-process address space: every task sees its 2MB window at USER_VA.
// The kernel half of each PML4 is shared from the master tables.
pub const USER_VA: u64 = 0x4000_0000;
pub const USER_SLOT: u64 = 2 * 1024 * 1024;
// Refcounted frame pool backing all user memory and page tables: 16MB of
// 4K frames under the kernel's 1:1 map (physical == virtual access).
pub const FRAME_BASE: u64 = 0x3F000000;
pub const FRAME_COUNT: usize = 4096;
pub const MASTER_PML4: u64 = 0x10000;

// Page flags
const PG_PRESENT: u64 = 0x1;
const PG_WRITE: u64 = 0x2;
const PG_USER: u64 = 0x4;
const PG_NX: u64 = 0x8000_0000_0000_0000;
const PG_SOFTW: u64 = 1 << 9; // software bit: "logically writable, COW-shared"
const PADDR_MASK: u64 = 0x000F_FFFF_FFFF_F000;

// ELF64 constants
const ELF_MAGIC: [u8; 4] = [0x7F, b'E', b'L', b'F'];
const PT_LOAD: u32 = 1;

pub const ST_FREE: u8 = 0;
pub const ST_READY: u8 = 1;
pub const ST_RUNNING: u8 = 2;
pub const ST_ZOMBIE: u8 = 3;
// legacy alias used by the GUI task manager for the dead/zombie color
pub const ST_DEAD: u8 = ST_ZOMBIE;
pub const ST_WAITING: u8 = 4;
pub const NO_OWNER: u32 = 0xFFFF_FFFF;

pub const SYS_WRITE: u64 = 1;
pub const SYS_YIELD: u64 = 2;
pub const SYS_EXIT: u64 = 3;
pub const SYS_FORK: u64 = 4;
pub const SYS_NICE: u64 = 5;
pub const SYS_EXEC: u64 = 6;
pub const SYS_GETPID: u64 = 7;
pub const SYS_KILL: u64 = 8;
pub const SYS_WAIT: u64 = 9;

// Signals (default actions only): INT/KILL/TERM are fatal, USR1/USR2/CHLD
// are ignored until user handlers exist.
pub const SIG_INT: u32 = 2;
pub const SIG_KILL: u32 = 9;
pub const SIG_USR1: u32 = 10;
pub const SIG_USR2: u32 = 12;
pub const SIG_TERM: u32 = 15;
pub const SIG_CHLD: u32 = 17;

fn sig_bit(s: u32) -> u32 {
    if s == 0 || s > 31 {
        0
    } else {
        1 << (s - 1)
    }
}

const SIG_FATAL: u32 = (1 << (SIG_INT - 1)) | (1 << (SIG_KILL - 1)) | (1 << (SIG_TERM - 1));
const SIG_IGNORE: u32 = (1 << (SIG_USR1 - 1)) | (1 << (SIG_USR2 - 1)) | (1 << (SIG_CHLD - 1));

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
    // --- phase D ---
    pub cr3: u64,      // per-process PML4 physical (0x10000 for kernel tasks)
    pub pt: u64,       // the task's user PT (one 4K page == 2MB window)
    pub ppid: u32,     // parent pid (0 = orphan)
    pub owner: u32,    // cpu index running this task (NO_OWNER = none)
    pub wait_pid: u32, // waitpid target (0 = any child)
    pub sigpend: u32,  // pending signal bitmap
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
    cr3: 0,
    pt: 0,
    ppid: 0,
    owner: NO_OWNER,
    wait_pid: 0,
    sigpend: 0,
};

pub static mut TASKS: [Task; MAX_TASKS] = [TASK_TEMPLATE; MAX_TASKS];

// One parked idle task per core; it is never picked by other cores (prio 0)
// and exists so a drained core has a kernel context to hlt in.
pub static mut IDLE_TASKS: [Task; crate::percpu::MAX_CPU] = [TASK_TEMPLATE; crate::percpu::MAX_CPU];

// private kernel stacks + parked frames for every core's idle task.
// NEVER place the idle frame on the core's bring-up stack: for APs that
// memory is LIVE at init time (init_idle would stomp return addresses and
// the AP dies before reaching idle_loop).
#[repr(align(16))]
#[derive(Clone, Copy)]
struct IdleStack([u8; 4096]);
static mut IDLE_STACKS: [IdleStack; crate::percpu::MAX_CPU] =
    [IdleStack([0; 4096]); crate::percpu::MAX_CPU];

pub fn idle_ptr(idx: usize) -> *mut Task {
    unsafe { &mut IDLE_TASKS[idx] as *mut Task }
}

#[repr(align(16))]
#[derive(Clone, Copy)]
struct KStack([u8; KSTACK_SIZE]);

static mut KSTACKS: [KStack; MAX_TASKS] = [KStack([0; KSTACK_SIZE]); MAX_TASKS];

static mut SCHED_READY: bool = false;
static mut NEXT_PID: u32 = 10;
pub static mut OUTPUT_DIRTY: bool = false;

// ---- locks -------------------------------------------------------------
// SCHED_LOCK serializes scheduler decisions across cores; FRAME_LOCK the
// frame allocator. Lock order is always SCHED -> FRAME, never reversed.
use core::sync::atomic::{AtomicBool, Ordering};

static SCHED_LOCK: AtomicBool = AtomicBool::new(false);
static FRAME_LOCK: AtomicBool = AtomicBool::new(false);

#[inline]
fn lock_sched() {
    let mut spins: u64 = 0;
    while SCHED_LOCK.swap(true, Ordering::Acquire) {
        core::hint::spin_loop();
        spins += 1;
        if spins == 1 << 30 {
            panic!("sched lock stuck: held by nobody (corrupted lock?) core {}", crate::percpu::cpu_idx());
        }
    }
}

#[inline]
fn unlock_sched() {
    SCHED_LOCK.store(false, Ordering::Release);
}

#[inline]
fn lock_frames() {
    let mut spins: u64 = 0;
    while FRAME_LOCK.swap(true, Ordering::Acquire) {
        core::hint::spin_loop();
        spins += 1;
        if spins == 1 << 30 {
            panic!("frame lock stuck: core {}", crate::percpu::cpu_idx());
        }
    }
}

#[inline]
fn unlock_frames() {
    FRAME_LOCK.store(false, Ordering::Release);
}

pub unsafe fn init() {
    enable_user_pages();
    // BSP idle context: a parked frame that schedule() can switch into when
    // the task it is leaving cannot resume (e.g. an orphan that just exited)
    init_idle(0, 0);
    let t = &mut TASKS[0];
    t.state = ST_RUNNING;
    t.pid = 1;
    t.slot = 0;
    t.is_user = false;
    t.kstack_top = 0;
    t.prio = 1;
    t.cr3 = MASTER_PML4;
    t.pt = 0;
    t.ppid = 0;
    t.owner = 0;
    t.wait_pid = 0;
    t.sigpend = 0;
    // the desktop kernel task runs at the lowest user priority: user tasks
    // always preempt it (old scheduler semantics), priority order among users
    let n = b"miluna-gui";
    t.name[..n.len()].copy_from_slice(n);
    crate::percpu::cur_set(t as *mut Task);
    SCHED_READY = true;
}

pub unsafe fn init_idle(idx: usize, _kstack_top: u64) {
    if idx >= crate::percpu::MAX_CPU {
        return;
    }
    let t = &mut IDLE_TASKS[idx] as *mut Task;
    (*t).state = ST_RUNNING;
    (*t).is_user = false;
    (*t).pid = 200 + idx as u32;
    (*t).slot = 0;
    (*t).prio = 0;
    (*t).cr3 = MASTER_PML4;
    (*t).pt = 0;
    (*t).owner = idx as u32;
    (*t).kstack_top = (IDLE_STACKS[idx].0.as_ptr() as u64) + 4096;
    let n = b"idle?";
    let mut name = [0u8; 16];
    for j in 0..n.len() {
        name[j] = n[j];
    }
    name[4] = b'0' + idx as u8;
    (*t).name = name;
    // A REAL parked kernel frame so schedule() can switch into the idle hlt
    // loop even when the task it is leaving cannot resume (zombie/waiting):
    // handle_trap's epilogue sees cs=0x08 and takes the iretq path.
    let kstack_top = (IDLE_STACKS[idx].0.as_ptr() as u64) + 4096;
    let fp = (kstack_top - 192) as *mut TrapFrame;
    core::ptr::write_bytes(fp as *mut u8, 0, 192);
    (*fp).rip = idle_loop as u64;
    (*fp).cs = 0x08;
    (*fp).rflags = 0x202;
    (*fp).rsp = kstack_top - 2048; // stack for the idle_loop call frame
    (*fp).ss = 0x10;
    (*t).saved_rsp = fp as u64;
    // valid FPU area (fxrstor in the trap epilogue loads it)
    core::ptr::write_bytes((*t).fpu.as_mut_ptr(), 0, 512);
    *((*t).fpu.as_mut_ptr() as *mut u16) = 0x037F;
    *((*t).fpu.as_mut_ptr().add(24) as *mut u32) = 0x1F80;
}

// Every core parks here when the queue is drained. Entered via iret from the
// parked idle frame (cs=0x08, interrupts on): schedule() hands out work from
// the resched IPI / timer path, or we fall straight back into hlt.
pub extern "C" fn idle_loop() -> ! {
    loop {
        unsafe {
            // sweep stuck in-service vectors: QEMU TCG drops EOI writes issued
            // from LAPIC-interrupt context (see lapic.rs) - a write from task
            // context does land, so the sweep unblocks later IPIs.
            //
            // The LAPIC one-shot is NOT armed here: every v=24 fires with the
            // EOI dropped by TCG, leaving the in-service bit stuck (PPR raised)
            // until the next task-context sweep - thousands of blocked-delivery
            // windows per second per core. Parked cores wake on resched IPIs
            // (class 0x30 pierces a stuck class-0x20 ISR) and the BSP PIT
            // scheduler steals parked work within one 20 ms tick. On real
            // hardware, arming a periodic timer here is correct - see lapic.rs.
            crate::lapic::eoi();
            { let b = b"I"; for &c in b { core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") c, options(nomem, nostack)); } }
            core::arch::asm!("sti", "hlt", options(nomem, nostack));
        }
    }
}

fn enable_user_pages() {
    // identity-map page tables from boot.asm: PML4@0x10000, PDPT@0x11000
    // set U/S on PML4[0] and PDPT[0] so per-task user trees are reachable
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

unsafe fn switch_cr3(target: u64) {
    if target == 0 {
        return;
    }
    let cur: u64;
    core::arch::asm!("mov {}, cr3", out(reg) cur, options(nomem, nostack));
    if cur != target {
        core::arch::asm!("mov cr3, {}", in(reg) target, options(nomem, nostack));
    }
}

// ---- refcounted frame pool --------------------------------------------
static mut REFCNT: [u16; FRAME_COUNT] = [0; FRAME_COUNT];
static mut FRAME_HINT: usize = 0;

fn frame_idx(phys: u64) -> usize {
    ((phys - FRAME_BASE) >> 12) as usize
}

// returns 0 on exhaustion
unsafe fn frame_alloc() -> u64 {
    lock_frames();
    let mut got: u64 = 0;
    for _ in 0..FRAME_COUNT {
        let i = FRAME_HINT;
        FRAME_HINT = (FRAME_HINT + 1) % FRAME_COUNT;
        if REFCNT[i] == 0 {
            REFCNT[i] = 1;
            got = FRAME_BASE + (i as u64) * 0x1000;
            break;
        }
    }
    unlock_frames();
    got
}

unsafe fn frame_ref(phys: u64) {
    let i = frame_idx(phys);
    if i < FRAME_COUNT {
        lock_frames();
        if REFCNT[i] < 0xFFFF {
            REFCNT[i] += 1;
        }
        unlock_frames();
    }
}

unsafe fn frame_unref(phys: u64) {
    let i = frame_idx(phys);
    if i < FRAME_COUNT {
        lock_frames();
        if REFCNT[i] > 0 {
            REFCNT[i] -= 1;
        }
        unlock_frames();
    }
}

unsafe fn frame_refcount(phys: u64) -> u16 {
    let i = frame_idx(phys);
    if i < FRAME_COUNT {
        core::ptr::read_volatile(&raw const REFCNT[i])
    } else {
        0
    }
}

// ---- per-process page trees --------------------------------------------
// Layout per task (all pages from the frame pool):
//   PML4[1..512]  = copy of master (kernel half, shared)
//   PML4[0]       -> PDPT (copy of master entries; [1] replaced below)
//   PDPT[0]       -> master PDT0 (kernel identity 0..1GB, kernel-only)
//   PDPT[1]       -> PDT (per-task), whose [0] -> PT describing USER_VA..+2MB
unsafe fn build_ptree() -> (u64, u64) {
    let pml4 = frame_alloc();
    let pdpt = frame_alloc();
    let pdt = frame_alloc();
    let pt = frame_alloc();
    if pml4 == 0 || pdpt == 0 || pdt == 0 || pt == 0 {
        return (0, 0);
    }
    core::ptr::write_bytes(pml4 as *mut u8, 0, 0x1000);
    core::ptr::write_bytes(pdpt as *mut u8, 0, 0x1000);
    core::ptr::write_bytes(pdt as *mut u8, 0, 0x1000);
    core::ptr::write_bytes(pt as *mut u8, 0, 0x1000);
    // kernel half verbatim from the master PML4 (entries 1..512)
    let m = MASTER_PML4 as *const u64;
    for i in 1..512 {
        core::ptr::write_volatile((pml4 as *mut u64).add(i), core::ptr::read_volatile(m.add(i)));
    }
    core::ptr::write_volatile(pml4 as *mut u64, pdpt | PG_PRESENT | PG_WRITE | PG_USER);
    // kernel low identity map shared (no U: user mode must not touch 0..1GB)
    let mpdpt = 0x11000 as *const u64;
    for i in 2..512 {
        core::ptr::write_volatile(
            (pdpt as *mut u64).add(i),
            core::ptr::read_volatile(mpdpt.add(i)),
        );
    }
    core::ptr::write_volatile(pdpt as *mut u64, 0x12000 | PG_PRESENT | PG_WRITE);
    core::ptr::write_volatile((pdpt as *mut u64).add(1), pdt | PG_PRESENT | PG_WRITE | PG_USER);
    core::ptr::write_volatile(pdt as *mut u64, pt | PG_PRESENT | PG_WRITE | PG_USER);
    (pml4, pt)
}

// COW variant: child gets its own tree, but every present parent page is
// SHARED read-only; writable pages get PG_SOFTW so a later write fault knows
// they are copy-on-write and not genuinely read-only (text/rodata).
unsafe fn build_ptree_from(pcr3: u64, ppt: u64) -> (u64, u64) {
    let (cr3, pt) = build_ptree();
    if cr3 == 0 {
        return (0, 0);
    }
    let _ = pcr3; // parent tree structure is identical; only the PT matters
    let src = ppt as *mut u64;
    let dst = pt as *mut u64;
    for i in 0..512 {
        let e = core::ptr::read_volatile(src.add(i));
        if e & PG_PRESENT == 0 {
            continue;
        }
        let frame = e & PADDR_MASK;
        frame_ref(frame);
        let mut fl = e & 0xFFF;
        if e & PG_WRITE != 0 {
            fl &= !PG_WRITE;
            fl |= PG_SOFTW;
        }
        core::ptr::write_volatile(dst.add(i), frame | fl);
        core::ptr::write_volatile(src.add(i), frame | fl); // demote parent too
    }
    flush_tlb(); // parent's TLB still caches writable translations
    (cr3, pt)
}

// free a task's user tree: unref every present page, then the 4 table pages
unsafe fn free_tree(t: *mut Task) {
    let cr3 = (*t).cr3;
    if cr3 == 0 || cr3 == MASTER_PML4 {
        return;
    }
    let pt = (*t).pt;
    if pt != 0 {
        let ptp = pt as *mut u64;
        for i in 0..512 {
            let e = core::ptr::read_volatile(ptp.add(i));
            if e & PG_PRESENT != 0 {
                frame_unref(e & PADDR_MASK);
            }
        }
        frame_unref(pt);
    }
    unsafe fn entry_at(a: u64, i: usize) -> u64 {
        core::ptr::read_volatile((a as *const u64).add(i))
    }
    let pdpt = entry_at(cr3, 0) & PADDR_MASK;
    if pdpt != 0 {
        let pdt = entry_at(pdpt, 1) & PADDR_MASK;
        if pdt != 0 {
            frame_unref(pdt);
        }
        frame_unref(pdpt);
    }
    frame_unref(cr3);
    (*t).cr3 = 0;
    (*t).pt = 0;
}

// Write fault into a COW page: copy the frame if shared, make it writable.
// Returns true when the fault is resolved and execution may resume.
pub unsafe fn cow_resolve(t: *mut Task, va: u64) -> bool {
    if va < USER_VA || va >= USER_VA + USER_SLOT {
        return false;
    }
    let pt = (*t).pt;
    if pt == 0 {
        return false;
    }
    let ptp = pt as *mut u64;
    let i = ((va >> 12) & 0x1FF) as usize;
    let e = core::ptr::read_volatile(ptp.add(i));
    if e & PG_PRESENT == 0 {
        return false;
    }
    if e & PG_WRITE != 0 {
        return true; // stale TLB entry - refetch will succeed
    }
    if e & PG_SOFTW == 0 {
        return false; // genuine R/O page (text/rodata): real fault
    }
    let frame = e & PADDR_MASK;
    let fl = PG_PRESENT | PG_WRITE | PG_USER | (e & PG_NX);
    if frame_refcount(frame) > 1 {
        let nf = frame_alloc();
        if nf == 0 {
            return false; // out of memory: fatal for the task
        }
        core::ptr::copy_nonoverlapping(frame as *const u8, nf as *mut u8, 0x1000);
        frame_unref(frame);
        core::ptr::write_volatile(ptp.add(i), nf | fl);
    } else {
        core::ptr::write_volatile(ptp.add(i), frame | fl);
    }
    flush_tlb();
    true
}

fn parse_u16(b: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([b[off], b[off + 1]])
}

fn parse_u32(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

fn parse_u64(b: &[u8], off: usize) -> u64 {
    u64::from_le_bytes([
        b[off], b[off + 1], b[off + 2], b[off + 3], b[off + 4], b[off + 5], b[off + 6], b[off + 7],
    ])
}

// ---- load-steal SMP scheduling ----------------------------------------
// One global run queue (the TASKS array) with per-task CPU affinity: a core
// first looks for work it owns (or unowned), then STEALS the highest-priority
// ready task from another core. Becoming READY is only safe because READY
// means "not running anywhere" - the pick runs under SCHED_LOCK. Idle cores
// are woken with a resched IPI (kick_idle / poke_owner).
pub fn schedule(f: &mut TrapFrame) -> u64 {
    schedule_inner(f, false)
}

// yield variant: if the pick comes back to the caller itself (nothing else
// ready), park on idle instead of resuming immediately - a yield hands the
// core to the idle loop, the LAPIC one-shot armed there wakes it in ~1ms.
pub fn schedule_yield(f: &mut TrapFrame) -> u64 {
    schedule_inner(f, true)
}

fn schedule_inner(f: &mut TrapFrame, yielder: bool) -> u64 {
    unsafe {
        if !SCHED_READY {
            return 0;
        }
        lock_sched();
        let cur = crate::percpu::cur();
        let me = crate::percpu::cpu_idx();
        (*cur).saved_rsp = f as *mut TrapFrame as u64;
        if (*cur).state == ST_RUNNING {
            (*cur).state = ST_READY;
        }
        reap_orphans();
        let mut next: *mut Task = core::ptr::null_mut();
        let mut stole = false;
        let start = if (*cur).slot < MAX_TASKS { (*cur).slot } else { 0 };
        if yielder {
            // Cooperative yield = full round-robin handoff: the next READY
            // task in slot order runs, REGARDLESS of priority. Strict
            // priority still governs timer-driven reschedules, so CPU-bound
            // high-prio tasks keep winning preemption slices - but the
            // desktop/shell task (lowest prio) regains a slice at every
            // user-task yield instead of starving until the queue drains.
            for k in 1..=MAX_TASKS {
                let i = (start + k) % MAX_TASKS;
                let t = &mut TASKS[i] as *mut Task;
                if (*t).state == ST_READY && (*t).prio >= 1 {
                    next = t;
                    break;
                }
            }
        } else {
            for pass in 0..2 {
                let mut best_prio = 0u32;
                let mut pick: *mut Task = core::ptr::null_mut();
                for k in 1..=MAX_TASKS {
                    let i = (start + k) % MAX_TASKS;
                    let t = &mut TASKS[i] as *mut Task;
                    if (*t).state != ST_READY || (*t).prio == 0 {
                        continue;
                    }
                    let owned = (*t).owner;
                    let ok = if pass == 0 {
                        owned == me as u32 || owned == NO_OWNER
                    } else {
                        owned != me as u32
                    };
                    if ok && (*t).prio > best_prio {
                        best_prio = (*t).prio;
                        pick = t;
                    }
                }
                if !pick.is_null() {
                    next = pick;
                    stole = pass == 1;
                    break;
                }
            }
        }
        if !next.is_null() {
            // DETECTOR: a READY task must not be cur on another core; if it
            // is, the state machine already broke somewhere - catch it here.
            for c in 0..crate::percpu::MAX_CPU {
                if c != me && crate::percpu::cur_of(c) == next {
                    panic!("double-run: task slot {} picked on core {} but cur on core {}", (*next).slot, me, c);
                }
            }
            (*next).owner = me as u32;
            (*next).state = ST_RUNNING;
            if stole {
                crate::percpu::bump(2);
            }
            crate::percpu::bump(3);
            crate::percpu::cur_set(next);
            e9_tag(if stole { b'S' } else { b'N' });
            unlock_sched();
            switch_cr3((*next).cr3);
            return (*next).saved_rsp;
        }
        // nothing pickable: keep a still-ready current, else park on the idle
        // task - unless the caller is yielding, then always park: a yield
        // hands the CPU over even when nothing else is runnable
        if !yielder && (*cur).prio == 0 {
            if (*cur).state == ST_READY {
                (*cur).state = ST_RUNNING;
            }
            unlock_sched();
            return 0;
        }
        if !yielder && (*cur).state == ST_READY {
            (*cur).state = ST_RUNNING;
            unlock_sched();
            return 0;
        }
        // the pick loop may have found no better task and picked the yielding
        // caller itself - treat that as "nothing else ready"
        if !next.is_null() && next == cur {
            next = core::ptr::null_mut();
        }
        let idle = idle_ptr(me);
        // a task that cannot resume (zombie/destroyed/waiting) must NEVER
        // return to its userspace frame - park the core on its idle loop
        let cur_resumable = (*cur).state == ST_RUNNING || (*cur).state == ST_READY;
        let idle_parked = ((*idle).state == ST_RUNNING || (*idle).state == ST_READY)
            && (*idle).saved_rsp != 0;
        if (!cur_resumable || yielder) && idle_parked && cur != idle {
            { let b = b"Y"; for &c in b { core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") c, options(nomem, nostack)); } }
            (*idle).state = ST_RUNNING;
            crate::percpu::cur_set(idle);
            unlock_sched();
            switch_cr3((*idle).cr3);
            return (*idle).saved_rsp;
        }
        if (*idle).state == ST_READY {
            (*idle).state = ST_RUNNING;
            crate::percpu::cur_set(idle);
            unlock_sched();
            switch_cr3((*idle).cr3);
            return (*idle).saved_rsp;
        }
        unlock_sched();
        0
    }
}

// Send a resched IPI to one idle core (called after work was published).
pub fn kick_idle() {
    unsafe {
        for c in 1..crate::percpu::online() {
            let idle = idle_ptr(c);
            if (*idle).state == ST_RUNNING && crate::percpu::cur_of(c) == idle {
                let apic = crate::percpu::CPUS[c].apic as u8;
                crate::lapic::send_ipi(apic, crate::lapic::VEC_RESCHED);
                return;
            }
        }
    }
}

// Nudge the core currently owning `owner` so it re-evaluates its queue.
pub fn poke_owner(owner: u32) {
    if owner == NO_OWNER {
        return;
    }
    unsafe {
        let c = owner as usize;
        if c < crate::percpu::MAX_CPU {
            let apic = crate::percpu::CPUS[c].apic as u8;
            crate::lapic::send_ipi(apic, crate::lapic::VEC_RESCHED);
        }
    }
}

// ---- process lifecycle: zombie, reap, waitpid ---------------------------
// Terminal state transition. If a parent is blocked in waitpid for this task
// its parked frame gets the (exit_code<<32|pid) result and the child is reaped
// immediately; otherwise the task lingers as a visible zombie until waitpid
// or until the orphan reaper frees it.
pub unsafe fn make_zombie(t: *mut Task, code: u32) {
    lock_sched();
    (*t).exit_code = code;
    (*t).state = ST_ZOMBIE;
    OUTPUT_DIRTY = true;
    for i in 0..MAX_TASKS {
        let p = &mut TASKS[i] as *mut Task;
        if p != t
            && (*p).state == ST_WAITING
            && (*p).pid == (*t).ppid
            && ((*p).wait_pid == 0 || (*p).wait_pid == (*t).pid)
        {
            let fp = (*p).saved_rsp as *mut TrapFrame;
            (*fp).rax = (((*t).exit_code as u64) << 32) | (*t).pid as u64;
            (*p).state = ST_READY;
            (*p).wait_pid = 0;
            (*p).sigpend |= sig_bit(SIG_CHLD);
            let owner = (*p).owner;
            unlock_sched();
            { let b = b"W"; for &c in b { core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") c, options(nomem, nostack)); } }
            poke_owner(owner);
            reap(t);
            return;
        }
    }
    // TRACE: wake scan found no waiter - dump why (child ppid, then for each
    // candidate parent: state digit)
    {
        let pp = (*t).ppid;
        core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") b'w', options(nomem, nostack));
        let mut v = pp;
        let mut digs = [0u8; 12];
        let mut n = 0;
        while v > 0 { digs[n] = b'0' + (v % 10) as u8; n += 1; v /= 10; }
        if n == 0 { core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") b'0', options(nomem, nostack)); }
        while n > 0 { n -= 1; core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") digs[n], options(nomem, nostack)); }
        for i in 0..MAX_TASKS {
            let p = &TASKS[i];
            if p.state != ST_FREE && p.pid == pp {
                core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") (b'0' + p.state), options(nomem, nostack));
            }
        }
    }
    unlock_sched();
    reap_orphans();
}

// free the address space and release the slot.
// NOTE: do NOT hold FRAME_LOCK across free_tree - it internally takes the
// same lock in frame_ref/frame_unref, and a spin lock re-acquired by its
// owner deadlocks forever (found the hard way: orphan reaping hung the
// whole box right after the fork demo's parent exited).
pub unsafe fn reap(t: *mut Task) {
    { let b = b"R"; for &c in b { core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") c, options(nomem, nostack)); } }
    free_tree(t);
    (*t).state = ST_FREE;
    OUTPUT_DIRTY = true;
}

// reap zombies whose parent is gone (orphaned); called opportunistically.
// NEVER reap a task that is the current context of some core: it may still be
// executing its exit syscall on that core - freeing its address space under
// it turns the core into a ghost that resumes user code with a destroyed
// page tree (observed as a 1M-fault storm with rip inside a call's payload).
unsafe fn reap_orphans() {
    for i in 0..MAX_TASKS {
        let t = &mut TASKS[i] as *mut Task;
        if (*t).state != ST_ZOMBIE {
            continue;
        }
        let mut is_cur = false;
        for c in 0..crate::percpu::MAX_CPU {
            if crate::percpu::cur_of(c) == t {
                is_cur = true;
                break;
            }
        }
        if is_cur {
            continue;
        }
        let mut alive = false;
        for j in 0..MAX_TASKS {
            let p = &TASKS[j];
            if j != i && p.state != ST_FREE && p.pid == (*t).ppid {
                alive = true;
                break;
            }
        }
        if !alive {
            reap(t);
        }
    }
}

// default-action signal delivery, called on the way back to ring 3.
// Returns true when the task died and the caller must re-schedule.
pub fn deliver_pending(f: &mut TrapFrame) -> bool {
    unsafe {
        let cur = crate::percpu::cur();
        if !(*cur).is_user || (*cur).state != ST_RUNNING {
            return false;
        }
        let p = (*cur).sigpend;
        if p == 0 {
            return false;
        }
        let fatal = p & SIG_FATAL;
        if fatal != 0 {
            (*cur).sigpend = 0;
            let sig = fatal.trailing_zeros() + 1;
            let mut n = 0usize;
            let mut put = |b: u8| {
                if n < 63 {
                    (*cur).msg[n] = b;
                    n += 1;
                }
            };
            let m = b"signal ";
            for &b in m {
                put(b);
            }
            put_dec(&mut put, sig as u64);
            (*cur).msg_len = n;
            OUTPUT_DIRTY = true;
            make_zombie(cur, (128 + sig) as u32);
            let _ = f;
            return true;
        }
        (*cur).sigpend = p & !SIG_IGNORE;
        false
    }
}

pub fn handle_syscall(f: &mut TrapFrame) -> u64 {
    unsafe {
        // task context: MMIO lands - EOI sweep keeps this core's LAPIC queue
        // unblocked (QEMU TCG drops EOI writes from interrupt context)
        crate::lapic::tick_service();
        let cur = crate::percpu::cur();
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
                return schedule_yield(f);
            }
            SYS_EXIT => {
                { let b = b"E"; for &c in b { core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") c, options(nomem, nostack)); } }
                make_zombie(cur, f.rdi as u32);
                return schedule(f);
            }
            SYS_FORK => {
                // hold the scheduler lock across slot allocation + child setup:
                // another core's scheduler must never observe (or double-claim)
                // this slot mid-initialization
                lock_sched();
                let mut slot = usize::MAX;
                for i in 1..MAX_TASKS {
                    if i != (*cur).slot && TASKS[i].state == ST_FREE {
                        slot = i;
                        break;
                    }
                }
                if slot == usize::MAX {
                    unlock_sched();
                    f.rax = u64::MAX;
                    return 0;
                }
                // child address space: own PML4, all present pages shared R/O
                let (ccr3, cpt) = build_ptree_from((*cur).cr3, (*cur).pt);
                if ccr3 == 0 {
                    unlock_sched();
                    f.rax = u64::MAX;
                    return 0;
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
                (*t).ppid = (*cur).pid;
                (*t).cr3 = ccr3;
                (*t).pt = cpt;
                (*t).owner = NO_OWNER;
                (*t).wait_pid = 0;
                (*t).sigpend = 0;
                (*t).name = (*cur).name;
                // child frame: copy of the parent's; per-process address spaces
                // mean user VAs are identical - no rebasing. rax = 0 in child.
                let fp = ((*t).kstack_top - 192) as *mut TrapFrame;
                core::ptr::copy_nonoverlapping(f as *const TrapFrame, fp, 1);
                (*fp).rax = 0;
                (*t).saved_rsp = fp as u64;
                OUTPUT_DIRTY = true;
                core::sync::atomic::compiler_fence(Ordering::SeqCst);
                (*t).state = ST_READY;
                unlock_sched();
                f.rax = (*t).pid as u64;
                kick_idle();
            }
            SYS_NICE => {
                (*cur).prio = (f.rdi as u32).clamp(1, 9);
                f.rax = (*cur).prio as u64;
            }
            SYS_GETPID => {
                f.rax = (*cur).pid as u64;
            }
            SYS_KILL => {
                let pid = f.rdi as u32;
                let sig = f.rsi as u32;
                let valid = sig_bit(sig) != 0
                    && (sig == SIG_INT
                        || sig == SIG_KILL
                        || sig == SIG_USR1
                        || sig == SIG_USR2
                        || sig == SIG_TERM
                        || sig == SIG_CHLD);
                let mut target: *mut Task = core::ptr::null_mut();
                for i in 0..MAX_TASKS {
                    let t = &mut TASKS[i] as *mut Task;
                    if (*t).state != ST_FREE && (*t).pid == pid {
                        target = t;
                        break;
                    }
                }
                if !valid || target.is_null() || !(*target).is_user {
                    f.rax = u64::MAX;
                    return 0;
                }
                if (*target).state == ST_ZOMBIE {
                    f.rax = 0; // POSIX: signalling a zombie is a no-op
                    return 0;
                }
                (*target).sigpend |= sig_bit(sig);
                { let b = b"K"; for &c in b { core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") c, options(nomem, nostack)); } }
                if (*target).state == ST_WAITING {
                    // a blocked task never traps: terminate it right away
                    make_zombie(target, 128 + sig);
                } else {
                    let owner = (*target).owner;
                    poke_owner(owner);
                }
                f.rax = 0;
            }
            SYS_WAIT => {
                let want = f.rdi as u32; // 0 or -1 = any child
                let mut found: *mut Task = core::ptr::null_mut();
                let mut zombie: *mut Task = core::ptr::null_mut();
                for i in 0..MAX_TASKS {
                    let c = &mut TASKS[i] as *mut Task;
                    if (*c).state != ST_FREE
                        && (*c).ppid == (*cur).pid
                        && (want == 0 || want == u32::MAX || (*c).pid == want)
                    {
                        if found.is_null() {
                            found = c;
                        }
                        if (*c).state == ST_ZOMBIE {
                            zombie = c;
                            break;
                        }
                    }
                }
                if !zombie.is_null() {
                    let code = (*zombie).exit_code;
                    let pid = (*zombie).pid;
                    reap(zombie);
                    f.rax = ((code as u64) << 32) | pid as u64;
                } else if !found.is_null() {
                    // block: wake-up happens in make_zombie when a child dies
                    (*cur).wait_pid = if want == u32::MAX { 0 } else { want };
                    (*cur).state = ST_WAITING;
                    return schedule(f);
                } else {
                    f.rax = u64::MAX;
                }
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
                if !elf_valid(bin) || load_elf(cur, bin) < 0 {
                    f.rax = u64::MAX;
                    return 0;
                }
                (*cur).msg_len = 0;
                for j in 0..16 {
                    (*cur).name[j] = if j < name.len() { name[j] } else { 0 };
                }
                // brand-new frame at the same kernel stack position: the old
                // user image is gone, execution continues inside the new one
                let fp = ((*cur).kstack_top - 192) as *mut TrapFrame;
                core::ptr::write_bytes(fp as *mut u8, 0, 192);
                (*fp).vector = 0;
                (*fp).slot1 = 0;
                (*fp).rip = USER_VA + parse_u64(bin, 0x18);
                (*fp).cs = USER_CS as u64;
                (*fp).rflags = 0x202;
                (*fp).rsp = (USER_VA + USER_SLOT - 64) as u64;
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
        let cur = crate::percpu::cur();
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
        let p4 = b" rip=";
        for &b in p4 {
            put(b);
        }
        put_dec(&mut put, f.rip);
        (*cur).msg_len = n;
    }
    unsafe { make_zombie(cur_pub(), (256 + vec) as u32) };
    schedule(f)
}

// TRACE: emit tag + core digit to the debugcon port (temporary)
pub fn e9_tag(tag: u8) {
    unsafe {
        let c = b'0' + crate::percpu::cpu_idx() as u8;
        core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") tag, options(nomem, nostack));
        core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") c, options(nomem, nostack));
    }
}

fn cur_pub() -> *mut Task {
    crate::percpu::cur()
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
        // serialize slot allocation + task setup against other cores' schedulers:
        // two simultaneous spawns must never grab the same FREE slot (TOCTOU)
        lock_sched();
        let mut slot = usize::MAX;
        for i in 1..MAX_TASKS {
            if TASKS[i].state == ST_FREE {
                slot = i;
                break;
            }
        }
        if slot == usize::MAX {
            unlock_sched();
            return -1;
        }
        let t = &mut TASKS[slot] as *mut Task;
        (*t).cr3 = 0; // no old tree to free
        if load_elf(t, bin) < 0 {
            (*t).state = ST_FREE;
            unlock_sched();
            return -3;
        }
        let entry = USER_VA + parse_u64(bin, 0x18);
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
        (*t).ppid = 0; // spawned from the kernel shell: no waiting parent
        (*t).owner = NO_OWNER;
        (*t).wait_pid = 0;
        (*t).sigpend = 0;
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
        fr.rsp = (USER_VA + USER_SLOT - 64) as u64;
        fr.ss = USER_SS as u64;
        (*t).saved_rsp = fp as u64;
        (*t).state = ST_READY;
        unlock_sched();
        kick_idle();
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

// Load an ELF64 image into a fresh per-process address space: one PT covers
// the 2MB window at USER_VA; every page comes from the frame pool and is
// mapped with per-page rights (text R+X, rodata R, data/stack RW, non-X NX).
// exec() calls this on a task with an existing tree: the old one is freed.
unsafe fn load_elf(t: *mut Task, bin: &[u8]) -> i32 {
    let e_entry = parse_u64(bin, 0x18);
    let phoff = parse_u64(bin, 0x20) as usize;
    let phentsize = parse_u16(bin, 0x36) as usize;
    let phnum = parse_u16(bin, 0x38) as usize;
    if e_entry >= (USER_SLOT - 64 * 1024) {
        return -1;
    }
    if (*t).cr3 != 0 && (*t).cr3 != MASTER_PML4 {
        free_tree(t); // exec: drop the previous image's memory
    }
    let (cr3, pt) = build_ptree();
    if cr3 == 0 {
        return -2;
    }
    (*t).cr3 = cr3;
    (*t).pt = pt;
    let ptp = pt as *mut u64;
    // stack: last 64KB of the window, RW + NX, fresh frames
    let stack_off = USER_SLOT - 64 * 1024;
    for i in (stack_off >> 12) as usize..512 {
        let fr = frame_alloc();
        if fr == 0 {
            return -2;
        }
        core::ptr::write_volatile(
            ptp.add(i),
            fr | PG_PRESENT | PG_WRITE | PG_USER | PG_NX,
        );
    }
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
        // union semantics for pages shared by several segments: writable if
        // any segment wants W, executable if any earlier mapping had X
        let mut fl = PG_PRESENT | PG_USER;
        if flags & 2 != 0 {
            fl |= PG_WRITE;
        }
        if flags & 1 == 0 {
            fl |= PG_NX;
        }
        let start = (vaddr >> 12) as usize;
        let end = (((vaddr + memsz) + 0xFFF) >> 12) as usize;
        for i in start..end.min(512) {
            let old = core::ptr::read_volatile(ptp.add(i));
            if old & PG_PRESENT != 0 {
                if old & PG_NX == 0 {
                    fl &= !PG_NX;
                }
                fl |= old & PG_WRITE;
            }
        }
        // map fresh frames, then fill them through the kernel 1:1 map
        for i in start..end.min(512) {
            let old = core::ptr::read_volatile(ptp.add(i));
            if old & PG_PRESENT == 0 {
                let fr = frame_alloc();
                if fr == 0 {
                    return -2;
                }
                core::ptr::write_volatile(ptp.add(i), fr | fl);
            }
        }
        let src = bin[off..].as_ptr();
        let mut done = 0usize;
        for i in start..end.min(512) {
            let pte = core::ptr::read_volatile(ptp.add(i));
            let fr = (pte & PADDR_MASK) as *mut u8;
            let vpage = (i as u64) << 12;
            let lo = if vaddr > vpage { (vaddr - vpage) as usize } else { 0 };
            let hi = ((vaddr + memsz) - vpage).min(0x1000) as usize;
            if hi <= lo {
                continue;
            }
            let n = hi - lo;
            let file_here = if (done as u64) < filesz {
                (filesz as usize - done).min(n)
            } else {
                0
            };
            if file_here > 0 {
                core::ptr::copy_nonoverlapping(src.add(done), fr.add(lo), file_here);
            }
            if file_here < n {
                core::ptr::write_bytes(fr.add(lo + file_here), 0, n - file_here);
            }
            done += n;
        }
    }
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
        ST_ZOMBIE => "Zombie",
        ST_WAITING => "Waiting",
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
        put_pad(b"CORE", 5);
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
            // owner core (or "-")
            if t.owner == NO_OWNER {
                put_pad(b"-", 5);
            } else {
                let mut cd = [0u8; 4];
                let mut cn = 0;
                let mut cv = t.owner as u64;
                if cv == 0 {
                    cd[0] = b'0';
                    cn = 1;
                }
                while cv > 0 && cn < 4 {
                    cd[cn] = b'0' + (cv % 10) as u8;
                    cv /= 10;
                    cn += 1;
                }
                let mut cr = [0u8; 4];
                for k in 0..cn {
                    cr[k] = cd[cn - 1 - k];
                }
                put_pad(&cr[..cn], 5);
            }
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
            if t.state == ST_ZOMBIE {
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


