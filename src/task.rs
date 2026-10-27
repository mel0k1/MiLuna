// Kernel tasks and ring 3 user processes with syscall/sysret and per-CPU
// preemptive scheduling: every CPU runs its own LAPIC timer tick and either
// preempts its current task or steals a ready one from the global pool.

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
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
// bit 9 is software-available in PTEs: marks a COW-shared (read-only) page
const PG_COW: u64 = 0x200;
const PG_ADDR: u64 = 0x000F_FFFF_FFFF_F000;

// ELF64 constants
const ELF_MAGIC: [u8; 4] = [0x7F, b'E', b'L', b'F'];
const PT_LOAD: u32 = 1;

pub const ST_FREE: u8 = 0;
pub const ST_READY: u8 = 1;
pub const ST_RUNNING: u8 = 2;
pub const ST_DEAD: u8 = 3;
pub const ST_WAITING: u8 = 4;

pub const SYS_WRITE: u64 = 1;
pub const SYS_YIELD: u64 = 2;
pub const SYS_EXIT: u64 = 3;
pub const SYS_FORK: u64 = 4;
pub const SYS_NICE: u64 = 5;
pub const SYS_EXEC: u64 = 6;
pub const SYS_GETPID: u64 = 7;
pub const SYS_WAIT: u64 = 8;

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
    pub parent_pid: u32,
    pub wait_pid: u32,
    pub owner: u8, // apic id of the CPU running this task, 0xFF = parked/stealable
    pub beat: u64, // millis() of the owning CPU's last scheduler tick
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
    parent_pid: 0,
    wait_pid: 0,
    owner: 0xFF,
    beat: 0,
};

pub static mut TASKS: [Task; MAX_TASKS] = [TASK_TEMPLATE; MAX_TASKS];

// Per-CPU scheduler data, indexed by APIC id (0..255). The trap/syscall
// entry paths resolve their CPU with one MMIO read of the LAPIC id register
// and index this table directly in assembly (stride = 32 bytes).
#[repr(C, align(8))]
#[derive(Clone, Copy)]
pub struct PerCpu {
    pub cur: u64, // *mut Task running on this CPU right now (0 = idle in hlt)
    pub rsp0_top: u64, // top of this CPU's RSP0 transit area
    pub scratch: u64, // syscall entry scratch (user rsp)
    pub cpu: u32, // logical cpu index for display
}

pub static mut PER_CPU: [PerCpu; 256] = [PerCpu {
    cur: 0,
    rsp0_top: 0,
    scratch: 0,
    cpu: 0,
}; 256];

// Address of the interrupt frame an idle CPU parked in is not used any
// more: every CPU parks in a synthesized kernel frame that irets into a
// rsp-pinned hlt loop (see idle_frame/cpu_idle_loop below).
pub static mut IDLE_TOPS: [u64; 256] = [0; 256];
static mut IDLE_FRAMES: [u64; 256] = [0; 256];

#[repr(align(16))]
#[derive(Clone, Copy)]
struct IdleStack([u8; 4096]);
static mut IDLE_STACKS: [IdleStack; MAX_TASKS] = [IdleStack([0; 4096]); MAX_TASKS];

// Idle loop for any CPU: rsp is re-pinned to this CPU's idle stack top on
// every pass (a same-privilege iretq pops but does not reload rsp, so the
// trap path leaks a few qwords per tick).
#[unsafe(naked)]
extern "C" fn cpu_idle_loop() -> ! {
    core::arch::naked_asm!(
        "2:",
        "mov eax, dword ptr [0xFEE00020]",
        "shr rax, 24",
        "shl rax, 3",
        "lea rcx, [rip + {tops}]",
        "mov rsp, [rcx + rax]",
        "hlt",
        "jmp 2b",
        tops = sym IDLE_TOPS,
    );
}

// Synthesized kernel-mode frame that lands a CPU in cpu_idle_loop. Built
// 1KB below the stack top: timer ticks taken while idle push their frames
// at the very top, so the idle frame must live out of their reach.
unsafe fn idle_frame(me: usize) -> u64 {
    let idx = PER_CPU[me].cpu as usize;
    if idx >= MAX_TASKS {
        return 0;
    }
    let top = (IDLE_STACKS[idx].0.as_ptr() as u64) + 4096;
    let fp = (top - 1024) as *mut u64;
    core::ptr::write_bytes(fp, 0, 24);
    *fp.add(18) = cpu_idle_loop as u64; // rip
    *fp.add(19) = 0x08; // cs (kernel)
    *fp.add(20) = 0x202; // rflags (IF set)
    *fp.add(21) = top - 8; // rsp
    *fp.add(22) = 0x10; // ss
    IDLE_TOPS[me] = top;
    fp as u64
}

// Copy-on-write reference counts, one per user-region 4K frame
// (MAX_TASKS slots * 512 pages). 0 = exclusive/unmapped.
static REFCNT: [AtomicU32; MAX_TASKS * 512] = [const { AtomicU32::new(0) }; MAX_TASKS * 512];

// Reset a wedged task: a task marked RUNNING whose owner CPU stopped
// ticking it (lost steal) is made stealable again. Runs from the BSP's
// once-per-second timer housekeeping - independent of any task. Safe:
// a stale beat means the owner's timer died BEFORE overwriting the task's
// saved_rsp, so the last parked frame is still valid for a re-steal.
static mut WATCHDOG_LAST: u64 = 0;
pub fn bsp_apic() -> u8 {
    unsafe { BSP_APIC }
}

pub unsafe fn wedged_tasks_reset() {
    let now = crate::timer::millis();
    if now.wrapping_sub(WATCHDOG_LAST) < 1000 {
        return;
    }
    WATCHDOG_LAST = now;
    for i in 0..MAX_TASKS {
        let t = &mut TASKS[i] as *mut Task;
        if (*t).state != ST_RUNNING || (*t).beat == 0 {
            continue;
        }
        if now.wrapping_sub((*t).beat) < 3000 {
            continue;
        }
        // find and clear the dead owner's registration
        for c in 0..256 {
            if PER_CPU[c].cur == t as u64 {
                PER_CPU[c].cur = 0;
                break;
            }
        }
        (*t).state = ST_READY;
        (*t).owner = 0xFF;
        (*t).beat = 0;
        OUTPUT_DIRTY = true;
    }
}

pub fn cpu_cur() -> *mut Task {
    unsafe { PER_CPU[crate::lapic::id() as usize].cur as *mut Task }
}

// register an AP's per-CPU area before it is woken (called from smp::init)
pub unsafe fn set_ap_cpu(cpu: u32, apic: u8, rsp0_top: u64) {
    PER_CPU[apic as usize].rsp0_top = rsp0_top;
    PER_CPU[apic as usize].cpu = cpu;
}

static SCHED_LOCK: AtomicBool = AtomicBool::new(false);

fn sched_lock() {
    while SCHED_LOCK
        .compare_exchange_weak(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        core::hint::spin_loop();
    }
}

fn sched_unlock() {
    SCHED_LOCK.store(false, Ordering::SeqCst);
}

#[repr(align(16))]
#[derive(Clone, Copy)]
struct KStack([u8; KSTACK_SIZE]);

static mut KSTACKS: [KStack; MAX_TASKS] = [KStack([0; KSTACK_SIZE]); MAX_TASKS];

static mut SCHED_READY: bool = false;
static mut NEXT_PID: u32 = 10;
static mut BSP_APIC: u8 = 0;
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
    // BSP per-CPU area: the GUI task starts out running here. LAPIC_BASE may
    // not be parsed yet - lapic::reg falls back to the default 0xFEE00000.
    let bsp = crate::lapic::id();
    BSP_APIC = bsp;
    PER_CPU[bsp as usize].cur = t as *mut Task as u64;
    PER_CPU[bsp as usize].rsp0_top = core::ptr::addr_of!(crate::gdt::RSP0_STACK) as u64 + 8192;
    PER_CPU[bsp as usize].cpu = 0;
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

// COW frame refcount helpers. Frame index = (phys - USER_BASE) >> 12.
unsafe fn frame_incref(phys: u64) {
    let idx = ((phys - USER_BASE as u64) >> 12) as usize;
    if idx < MAX_TASKS * 512 {
        REFCNT[idx].fetch_add(1, Ordering::SeqCst);
    }
}

unsafe fn frame_decref(phys: u64) -> u32 {
    let idx = ((phys - USER_BASE as u64) >> 12) as usize;
    if idx < MAX_TASKS * 512 {
        REFCNT[idx].fetch_sub(1, Ordering::SeqCst)
    } else {
        0
    }
}

// Drop COW references held by a slot's page table (task death or slot
// recycling). Only PRESENT entries hold references to other pages; entries
// with PG_COW but present=0 are clone-target markers for this slot's own
// pages and pin nothing.
unsafe fn drop_slot_pt(slot: usize) -> u32 {
    let pt = (USER_PT_BASE + slot * 0x1000) as *mut u64;
    let mut refs = 0u32;
    for i in 0..512 {
        let e = core::ptr::read_volatile(pt.add(i));
        if e & PG_PRESENT != 0 && e & PG_COW != 0 {
            let _ = frame_decref(e & PG_ADDR);
            refs += 1;
        }
    }
    refs
}

// release a dying task's COW references so its slot can be recycled and
// the pages it shared with fork relatives get unpinned
unsafe fn task_release_cow(slot: usize) {
    let _ = drop_slot_pt(slot);
}

// a dead slot whose pages are still COW-referenced by living fork children
// must not be recycled until those references are gone (the child would
// otherwise execute rewritten physical pages)
unsafe fn slot_reusable(slot: usize) -> bool {
    let pt = (USER_PT_BASE + slot * 0x1000) as *mut u64;
    for i in 0..512 {
        let e = core::ptr::read_volatile(pt.add(i));
        if e & PG_PRESENT != 0 && e & PG_COW != 0 {
            let idx = (((e & PG_ADDR) - USER_BASE as u64) >> 12) as usize;
            if idx < MAX_TASKS * 512 && REFCNT[idx].load(Ordering::SeqCst) > 0 {
                return false;
            }
        }
    }
    true
}

// Page fault fixup for copy-on-write pages. Called from the #PF path for
// ring 3 write faults: CR2 selects the slot/page, the PTE must be present
// with PG_COW set. Clones the frame when it is still shared, otherwise
// just restores writability. Returns true when the fault was handled.
pub fn cow_fix(f: &mut TrapFrame) -> bool {
    if f.slot1 & 2 == 0 {
        return false; // not a write fault
    }
    let cr2: u64;
    unsafe {
        core::arch::asm!("mov {}, cr2", out(reg) cr2, options(nomem, nostack));
    }
    if cr2 < USER_BASE as u64 || cr2 >= (USER_BASE + MAX_TASKS * USER_SLOT) as u64 {
        return false;
    }
    let slot = ((cr2 as usize - USER_BASE) / USER_SLOT) as usize;
    let idx = ((cr2 >> 12) & 0x1FF) as usize;
    unsafe {
        let pt = (USER_PT_BASE + slot * 0x1000) as *mut u64;
        let pte = core::ptr::read_volatile(pt.add(idx));
        if pte & PG_PRESENT == 0 || pte & PG_COW == 0 {
            return false;
        }
        let phys = pte & PG_ADDR;
        // a fresh frame for this task comes from its own slot: a PT index is
        // free when it was never used (present=0, no marker). Cloned-to
        // pages carry a PG_COW marker with present=0 so they are skipped.
        let mut free_i = usize::MAX;
        for i in 0..512 {
            if core::ptr::read_volatile(pt.add(i)) & (PG_PRESENT | PG_COW) == 0 {
                free_i = i;
                break;
            }
        }
        if free_i == usize::MAX {
            return false; // slot completely full - kill via caller
        }
        let fidx = ((phys - USER_BASE as u64) >> 12) as usize;
        let newfl = (pte & (PG_NX | PG_USER)) | PG_PRESENT | PG_WRITE;
        if fidx < MAX_TASKS * 512 && REFCNT[fidx].load(Ordering::SeqCst) > 1 {
            // the other fork side still maps the old frame: clone it into a
            // free page of this slot and mark that PT entry as taken
            REFCNT[fidx].fetch_sub(1, Ordering::SeqCst);
            let nphys = (USER_BASE + slot * USER_SLOT + free_i * 0x1000) as u64;
            core::ptr::copy_nonoverlapping(phys as *const u8, nphys as *mut u8, 0x1000);
            core::ptr::write_volatile(pt.add(free_i), nphys | PG_COW); // marker
            core::ptr::write_volatile(pt.add(idx), nphys | newfl);
        } else {
            // last reference (or uncounted): reuse the frame in place
            if fidx < MAX_TASKS * 512 {
                REFCNT[fidx].store(0, Ordering::SeqCst);
            }
            core::ptr::write_volatile(pt.add(idx), phys | newfl);
        }
        flush_tlb();
    }
    true
}

// Wake a task blocked in wait() because `dead` (its child) just exited.
// The exit code is written directly into the waiter's parked syscall frame,
// so it resumes as if wait() had returned. The zombie is reaped here.
unsafe fn wake_waiters(dead: *mut Task) {
    for i in 0..MAX_TASKS {
        let w = &mut TASKS[i] as *mut Task;
        if (*w).state != ST_WAITING || (*w).pid != (*dead).parent_pid {
            continue;
        }
        if (*w).wait_pid != 0 && (*w).wait_pid != (*dead).pid {
            continue;
        }
        let fp = (*w).saved_rsp as *mut TrapFrame;
        (*fp).rax = (*dead).exit_code as u64;
        (*w).wait_pid = 0;
        (*w).state = ST_READY;
        (*w).owner = 0xFF; // unowned: any CPU may pick the woken waiter
        (*dead).state = ST_FREE;
        OUTPUT_DIRTY = true;
        break;
    }
}

// Per-CPU preemptive scheduler with work stealing. Called from the LAPIC
// timer path on every CPU and from yield/exit/wait syscalls. All callers
// run with interrupts off; the spinlock guards the global ready set.
//
// Ownership discipline: a task is owned by exactly the CPU whose PER_CPU.cur
// points at it (Task.owner mirrors that apic id). A CPU disowns its task at
// the start of every scheduling decision and re-picks from the READY + unowned
// pool - so idle CPUs can safely steal work that another CPU just parked.
// A task is never picked while it is RUNNING on another CPU. When a CPU has
// nothing to run (its task died, or it was idle already) it parks in a
// synthesized kernel frame that lands in the rsp-pinned hlt loop.
pub fn schedule(f: &mut TrapFrame) -> u64 {
    unsafe {
        if !SCHED_READY {
            return 0;
        }
        sched_lock();
        let me = crate::lapic::id() as usize;
        let me8 = me as u8;
        let cur = PER_CPU[me].cur as *mut Task;
        if cur.is_null() {
            // idle CPU tick: the interrupted frame is the hlt loop frame
        } else {
            (*cur).saved_rsp = f as *mut TrapFrame as u64;
            if (*cur).state == ST_RUNNING {
                (*cur).state = ST_READY;
            }
            // disown: from here on the task is parked and stealable
            (*cur).owner = 0xFF;
            PER_CPU[me].cur = 0;
        }
        // highest priority among READY, unowned tasks (0 = none ready).
        // Slot 0 (miluna-gui, kernel task on the boot stack) is pinned to the
        // BSP: its frames live on the boot stack, so migrating it to an AP is
        // not safe. Only the BSP ever picks it.
        let mut best: u32 = 0;
        for i in 0..MAX_TASKS {
            if i == 0 && me8 != BSP_APIC {
                continue;
            }
            if TASKS[i].state == ST_READY && TASKS[i].owner == 0xFF && TASKS[i].prio > best {
                best = TASKS[i].prio;
            }
        }
        if best == 0 {
            if !cur.is_null() && (*cur).state == ST_READY {
                // keep running what we were interrupted in
                (*cur).owner = me8;
                (*cur).state = ST_RUNNING;
                PER_CPU[me].cur = cur as u64;
                sched_unlock();
                return 0;
            }
            // nothing to run (current task died, or idle CPU, nothing to steal)
            let idle = idle_frame(me);
            sched_unlock();
            return idle;
        }
        // round-robin among tasks with the top priority, starting after the
        // current slot; idle CPUs scan from slot 0 (steal path)
        let start = if cur.is_null() { MAX_TASKS - 1 } else { (*cur).slot };
        let mut next: *mut Task = core::ptr::null_mut();
        for k in 1..=MAX_TASKS {
            let i = (start + k) % MAX_TASKS;
            if i == 0 && me8 != BSP_APIC {
                continue;
            }
            if TASKS[i].state == ST_READY && TASKS[i].owner == 0xFF && TASKS[i].prio == best {
                next = &mut TASKS[i] as *mut Task;
                break;
            }
        }
        if next.is_null() {
            if !cur.is_null() && (*cur).state == ST_READY {
                (*cur).owner = me8;
                (*cur).state = ST_RUNNING;
                PER_CPU[me].cur = cur as u64;
                sched_unlock();
                return 0;
            }
            let idle = idle_frame(me);
            sched_unlock();
            return idle;
        }
        (*next).owner = me8;
        (*next).state = ST_RUNNING;
        if next != cur {
            // a full CR3 reload on every switch keeps TLBs coherent for
            // task migration between CPUs (spawn/fork/exec/COW all rewrite
            // page tables on the CPU that ran the caller)
            flush_tlb();
        }
        PER_CPU[me].cur = next as u64;
        sched_unlock();
        (*next).saved_rsp
    }
}

pub fn handle_syscall(f: &mut TrapFrame) -> u64 {
    unsafe {
        let cur = cpu_cur();
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
                // release this task's COW references: pages shared with fork
                // relatives get unpinned, the slot becomes recyclable
                unsafe { task_release_cow((*cur).slot) };
                // wake a parent blocked in wait(); it reaps the zombie and
                // receives the exit code in its parked frame
                wake_waiters(cur);
                return schedule(f);
            }
            SYS_FORK => {
                let cslot = (*cur).slot;
                let mut slot = usize::MAX;
                for i in 1..MAX_TASKS {
                    if i != cslot
                        && (TASKS[i].state == ST_FREE
                            || (TASKS[i].state == ST_DEAD && slot_reusable(i)))
                    {
                        slot = i;
                        break;
                    }
                }
                if slot == usize::MAX {
                    f.rax = u64::MAX;
                    return 0;
                }
                // copy-on-write fork: the child's PT points at the PARENT's
                // physical pages - every present page (code, rodata, data,
                // stack) is shared read-only with PG_COW set and gains a
                // reference; the first write from either side faults and
                // cow_fix() clones the page. The reference counts also pin
                // the parent's physical pages while the child is alive.
                let pt_src = (USER_PT_BASE + cslot * 0x1000) as *mut u64;
                let pt_dst = (USER_PT_BASE + slot * 0x1000) as *mut u64;
                // the slot may belong to a dead task whose PT still holds COW
                // references - release them before zeroing
                let _ = drop_slot_pt(slot);
                core::ptr::write_bytes(pt_dst as *mut u8, 0, 0x1000);
                // share EVERY present page (code, rodata, data, stack):
                // writable pages lose W and gain PG_COW on both sides,
                // read-only pages just gain the marker - the refcount pins
                // the parent's physical pages while the child is alive
                for i in 0..512 {
                    let e = core::ptr::read_volatile(pt_src.add(i));
                    if e & PG_PRESENT == 0 {
                        continue;
                    }
                    let sh = (e & !PG_WRITE) | PG_COW;
                    core::ptr::write_volatile(pt_src.add(i), sh);
                    core::ptr::write_volatile(pt_dst.add(i), sh);
                    frame_incref(e & PG_ADDR);
                }
                let pdt = 0x12000 as *mut u64;
                core::ptr::write_volatile(
                    pdt.add(504 + slot),
                    ((USER_PT_BASE + slot * 0x1000) as u64) | PG_PRESENT | PG_WRITE | PG_USER,
                );
                flush_tlb();
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
                (*t).parent_pid = (*cur).pid;
                (*t).wait_pid = 0;
                (*t).owner = 0xFF; // parked, ready to be picked by any CPU
                // child frame: copy of the parent's; the syscall entry stored the
                // RETURN address in rip (past the syscall instruction) already,
                // so the child resumes right after fork() with rax = 0.
                // rip and rsp are parent-slot linear addresses - rebase them to the
                // child's slot; the COW page tables map them onto parent pages
                // until either side writes.
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
            SYS_WAIT => {
                // rdi = child pid to wait for, 0 = any child. If the child is
                // already dead, reap it and return its code; otherwise block
                // until exit() (or kill_fault) wakes this task.
                let want = f.rdi as u32;
                let mut dead: *mut Task = core::ptr::null_mut();
                let mut have_child = false;
                for i in 1..MAX_TASKS {
                    let t = &mut TASKS[i] as *mut Task;
                    if (*t).state == ST_FREE || (*t).parent_pid != (*cur).pid {
                        continue;
                    }
                    if want != 0 && (*t).pid != want {
                        continue;
                    }
                    if (*t).state == ST_DEAD {
                        dead = t;
                        break;
                    }
                    have_child = true;
                }
                if !dead.is_null() {
                    f.rax = (*dead).exit_code as u64;
                    (*dead).state = ST_FREE; // reaped, slot reusable
                    OUTPUT_DIRTY = true;
                    return 0;
                }
                if !have_child {
                    f.rax = u64::MAX; // no such child
                    return 0;
                }
                (*cur).wait_pid = want;
                (*cur).state = ST_WAITING;
                OUTPUT_DIRTY = true;
                return schedule(f);
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
                // a task with COW children shares its physical pages with
                // them - exec would rewrite that code out from under them
                if !slot_reusable(slot) {
                    f.rax = u64::MAX;
                    return 0;
                }
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
        let cur = cpu_cur();
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
        // a faulting task is dead too: release its COW references and hand
        // its exit code to a waiting parent
        unsafe { task_release_cow((*cur).slot) };
        wake_waiters(cur);
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
            if TASKS[i].state == ST_FREE || (TASKS[i].state == ST_DEAD && unsafe { slot_reusable(i) }) {
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
        (*t).parent_pid = 0; // spawned by the kernel, nobody waits on these
        (*t).wait_pid = 0;
        (*t).owner = 0xFF; // parked, ready to be picked by any CPU
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
    // release COW references the old image still holds (exec in place,
    // or a dead task's slot being recycled)
    drop_slot_pt(slot);
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
        put_pad(b"CPU", 4);
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
            // which CPU runs this task right now (0..3), '-' otherwise
            let mut cpu_c = b'-';
            let tp = &TASKS[i] as *const Task as u64;
            for c in 0..256 {
                if PER_CPU[c].cur == tp {
                    cpu_c = b'0' + PER_CPU[c].cpu as u8;
                    break;
                }
            }
            put_pad(&[cpu_c], 4);
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
                // faulted tasks carry their fault details in msg ("fault#14
                // err=6 cr2=...") - show them, plain exits just get the code
                let is_fault = t.exit_code >= 256 && t.msg_len > 0 && t.msg[0] == b'f';
                if is_fault {
                    let mlen = t.msg_len.min(64);
                    let mut m = [0u8; 64];
                    for k in 0..mlen {
                        m[k] = if t.msg[k] == 0 { b' ' } else { t.msg[k] };
                    }
                    put_pad(&m[..mlen], 40);
                } else {
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
                }
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
