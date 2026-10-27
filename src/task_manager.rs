

use crate::graphics::{Graphics, Color};


#[derive(Clone, Copy)]
pub struct Process {
    pub pid: usize,
    pub name: [u8; 32],
    pub name_len: usize,
    pub memory: usize,
    pub cpu: usize,
    pub state: ProcessState,
}

#[derive(Clone, Copy, PartialEq)]
pub enum ProcessState {
    Running,
    Sleeping,
    Zombie,
    Stopped,
}

impl ProcessState {
    pub fn to_str(&self) -> &'static str {
        match self {
            ProcessState::Running => "Running",
            ProcessState::Sleeping => "Sleeping",
            ProcessState::Zombie => "Zombie",
            ProcessState::Stopped => "Stopped",
        }
    }
    
    pub fn color(&self) -> u32 {
        match self {
            ProcessState::Running => Color::rgb(0, 200, 0).to_u32(),
            ProcessState::Sleeping => Color::rgb(200, 200, 0).to_u32(),
            ProcessState::Zombie => Color::rgb(200, 0, 0).to_u32(),
            ProcessState::Stopped => Color::rgb(200, 100, 0).to_u32(),
        }
    }
}


static mut PROCESSES: [Process; 32] = [Process {
    pid: 0,
    name: [0; 32],
    name_len: 0,
    memory: 0,
    cpu: 0,
    state: ProcessState::Stopped,
}; 32];
static mut PROCESS_COUNT: usize = 0;
static mut PROCESS_TICK: usize = 0;
static mut CPU_LOAD: usize = 5;


fn num_to_str(num: usize) -> &'static str {
    static mut BUF: [u8; 10] = [0; 10];
    unsafe {
        let mut temp = num;
        let mut digits = [0u8; 10];
        let mut i = 0;
        if temp == 0 {
            BUF[0] = b'0';
            BUF[1] = 0;
            return "0";
        }
        while temp > 0 {
            digits[i] = b'0' + (temp % 10) as u8;
            temp /= 10;
            i += 1;
        }
        let mut j = 0;
        while i > 0 {
            i -= 1;
            BUF[j] = digits[i];
            j += 1;
        }
        BUF[j] = 0;
        core::str::from_utf8(&BUF[..j]).unwrap_or("0")
    }
}


pub fn init_task_manager() {
    unsafe {
        PROCESS_COUNT = 0;
        
        add_process("kernel", 2048, 2, ProcessState::Running);
        add_process("window_manager", 4096, 5, ProcessState::Running);
        add_process("display_server", 2048, 3, ProcessState::Running);
        add_process("input_driver", 512, 1, ProcessState::Running);
        add_process("ata_driver", 1024, 0, ProcessState::Sleeping);
        add_process("filesystem", 1536, 1, ProcessState::Sleeping);
        add_process("gui_shell", 2048, 4, ProcessState::Running);
        add_process("terminal", 512, 2, ProcessState::Running);
        add_process("task_manager", 1024, 0, ProcessState::Sleeping);
        add_process("mouse_driver", 256, 1, ProcessState::Running);
        add_process("vga_driver", 1024, 2, ProcessState::Running);
        add_process("keyboard_driver", 256, 1, ProcessState::Running);
    }
}


pub fn add_process(name: &str, memory: usize, cpu: usize, state: ProcessState) {
    unsafe {
        if PROCESS_COUNT >= 32 { return; }
        
        let bytes = name.as_bytes();
        let len = bytes.len().min(31);
        PROCESSES[PROCESS_COUNT].pid = PROCESS_COUNT + 1000;
        PROCESSES[PROCESS_COUNT].name[..len].copy_from_slice(&bytes[..len]);
        PROCESSES[PROCESS_COUNT].name[len] = 0;
        PROCESSES[PROCESS_COUNT].name_len = len;
        PROCESSES[PROCESS_COUNT].memory = memory;
        PROCESSES[PROCESS_COUNT].cpu = cpu;
        PROCESSES[PROCESS_COUNT].state = state;
        PROCESS_COUNT += 1;
    }
}


pub fn update_process_stats() {
    unsafe {
        PROCESS_TICK += 1;
        

        if PROCESS_TICK % 3 == 0 {
            let change = (PROCESS_TICK * 7) % 11;
            if change < 5 && CPU_LOAD > 3 {
                CPU_LOAD -= 1;
            } else if CPU_LOAD < 15 {
                CPU_LOAD += 1;
            }
        }
        

        if PROCESS_TICK % 2 == 0 {
            for i in 0..PROCESS_COUNT {

                if PROCESSES[i].cpu > 0 {
                    let change = (PROCESS_TICK * (i + 1) * 3) % 5;
                    if change < 2 && PROCESSES[i].cpu > 1 {
                        PROCESSES[i].cpu -= 1;
                    } else if PROCESSES[i].cpu < 8 {
                        PROCESSES[i].cpu += 1;
                    }
                }
                

                if PROCESS_TICK % 20 == 0 && i % 4 == 0 {
                    match PROCESSES[i].state {
                        ProcessState::Running => {
                            if PROCESS_TICK % 40 == 0 {
                                PROCESSES[i].state = ProcessState::Sleeping;
                            }
                        }
                        ProcessState::Sleeping => {
                            if PROCESS_TICK % 30 == 0 {
                                PROCESSES[i].state = ProcessState::Running;
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }
}


pub fn draw_task_manager(gfx: &mut Graphics, win_x: usize, win_y: usize, win_w: usize, win_h: usize) {
    let bg_color = Color::rgb(30, 30, 35).to_u32();
    gfx.fill_rect(win_x, win_y, win_w, win_h, bg_color);

    gfx.fill_rect(win_x, win_y, win_w, 30, Color::rgb(50, 50, 55).to_u32());
    gfx.draw_text(win_x + 10, win_y + 8, "Task Manager", Color::rgb(200, 200, 220).to_u32(), Color::rgb(50, 50, 55).to_u32());

    let (count, user_count, total_ticks, user_ticks) = crate::task::stats();
    let count_str = num_to_str(count);
    let user_str = num_to_str(user_count);
    gfx.draw_text(win_x + win_w - 190, win_y + 8, "Tasks: ", Color::rgb(150, 150, 180).to_u32(), Color::rgb(50, 50, 55).to_u32());
    gfx.draw_text(win_x + win_w - 130, win_y + 8, count_str, Color::rgb(200, 200, 220).to_u32(), Color::rgb(50, 50, 55).to_u32());
    gfx.draw_text(win_x + win_w - 100, win_y + 8, "ring3: ", Color::rgb(150, 150, 180).to_u32(), Color::rgb(50, 50, 55).to_u32());
    gfx.draw_text(win_x + win_w - 45, win_y + 8, user_str, Color::rgb(200, 200, 220).to_u32(), Color::rgb(50, 50, 55).to_u32());

    let cpu_pct = if total_ticks > 0 {
        (user_ticks * 100 / total_ticks) as usize
    } else {
        0
    };

    let stats_y = win_y + 40;
    gfx.fill_rect(win_x + 10, stats_y, win_w - 20, 22, Color::rgb(40, 40, 45).to_u32());
    gfx.draw_text(win_x + 20, stats_y + 4, "CPU (ring3 share): ", Color::rgb(150, 150, 180).to_u32(), Color::rgb(40, 40, 45).to_u32());
    let cpu_str = num_to_str(cpu_pct);
    gfx.draw_text(win_x + 175, stats_y + 4, cpu_str, Color::rgb(200, 200, 220).to_u32(), Color::rgb(40, 40, 45).to_u32());
    gfx.draw_text(win_x + 215, stats_y + 4, "%", Color::rgb(150, 150, 180).to_u32(), Color::rgb(40, 40, 45).to_u32());
    let tt_str = num_to_str(total_ticks as usize);
    gfx.draw_text(win_x + 260, stats_y + 4, "timer ticks: ", Color::rgb(150, 150, 180).to_u32(), Color::rgb(40, 40, 45).to_u32());
    gfx.draw_text(win_x + 370, stats_y + 4, tt_str, Color::rgb(200, 200, 220).to_u32(), Color::rgb(40, 40, 45).to_u32());

    let header_y = win_y + 70;
    gfx.fill_rect(win_x + 10, header_y, win_w - 20, 22, Color::rgb(60, 60, 70).to_u32());
    let headers = ["PID", "Name", "State", "Ticks", "Last message"];
    let col_x = [win_x + 18, win_x + 68, win_x + 168, win_x + 240, win_x + 320];
    for (i, header) in headers.iter().enumerate() {
        gfx.draw_text(col_x[i], header_y + 5, header, Color::rgb(180, 180, 200).to_u32(), Color::rgb(60, 60, 70).to_u32());
    }

    let row_height = 24;
    let mut y_pos = header_y + 28;
    let max_rows = (win_h.saturating_sub(140)) / row_height;
    let mut shown = 0usize;

    for i in 0..crate::task::MAX_TASKS {
        if shown >= max_rows { break; }
        let info = match crate::task::snapshot(i) {
            Some(x) => x,
            None => continue,
        };
        let row_color = if shown % 2 == 0 {
            Color::rgb(40, 40, 45).to_u32()
        } else {
            Color::rgb(35, 35, 40).to_u32()
        };
        gfx.fill_rect(win_x + 10, y_pos, win_w - 20, row_height, row_color);

        let pid_str = num_to_str(info.pid as usize);
        gfx.draw_text(col_x[0], y_pos + 5, pid_str, Color::rgb(150, 150, 180).to_u32(), row_color);

        let mut name_bytes = [0u8; 17];
        let mut nlen = 0usize;
        for j in 0..16 {
            if info.name[j] == 0 { break; }
            name_bytes[j] = info.name[j];
            nlen += 1;
        }
        let name = core::str::from_utf8(&name_bytes[..nlen]).unwrap_or("task");
        let name_color = if info.state == crate::task::ST_DEAD {
            Color::rgb(120, 120, 120).to_u32()
        } else {
            Color::rgb(200, 200, 220).to_u32()
        };
        gfx.draw_text(col_x[1], y_pos + 5, name, name_color, row_color);

        let state_str = crate::task::state_name(info.state);
        let state_color = match info.state {
            crate::task::ST_RUNNING => Color::rgb(0, 220, 0).to_u32(),
            crate::task::ST_READY => Color::rgb(220, 220, 0).to_u32(),
            crate::task::ST_DEAD => Color::rgb(220, 60, 60).to_u32(),
            _ => Color::rgb(150, 150, 180).to_u32(),
        };
        gfx.draw_text(col_x[2], y_pos + 5, state_str, state_color, row_color);

        let ticks_str = num_to_str(info.ticks as usize);
        gfx.draw_text(col_x[3], y_pos + 5, ticks_str, Color::rgb(200, 200, 220).to_u32(), row_color);

        let mut msg_bytes = [0u8; 68];
        let mut mlen;
        if info.state == crate::task::ST_DEAD {
            let ec = num_to_str(info.exit_code as usize);
            let mut k = 0usize;
            for &c in b"exit " { msg_bytes[k] = c; k += 1; }
            for &c in ec.as_bytes() { if k < 64 { msg_bytes[k] = c; k += 1; } }
            mlen = k;
        } else {
            mlen = info.msg_len.min(64);
            for j in 0..mlen {
                let c = info.msg[j];
                msg_bytes[j] = if c == 0 { b' ' } else { c };
            }
        }
        let msg = core::str::from_utf8(&msg_bytes[..mlen]).unwrap_or("");
        let msg_color = if info.state == crate::task::ST_DEAD {
            Color::rgb(220, 100, 100).to_u32()
        } else {
            Color::rgb(140, 200, 140).to_u32()
        };
        gfx.draw_text(col_x[4], y_pos + 5, msg, msg_color, row_color);

        y_pos += row_height;
        shown += 1;
    }

    if shown == 0 {
        gfx.draw_text(win_x + 20, y_pos + 5, "no user tasks - run `hello` in Terminal", Color::rgb(130, 130, 160).to_u32(), bg_color);
        y_pos += row_height;
    }

    let hint_y = win_y + win_h - 28;
    gfx.fill_rect(win_x + 10, hint_y, win_w - 20, 20, Color::rgb(45, 45, 52).to_u32());
    gfx.draw_text(win_x + 20, hint_y + 4, "Terminal: hello | hello fault | tasks", Color::rgb(130, 130, 160).to_u32(), Color::rgb(45, 45, 52).to_u32());
}

pub fn task_start() -> &'static str {
    unsafe {
        if PROCESS_COUNT == 0 {
            init_task_manager();
        }
        "Task Manager ready!"
    }
}

pub fn task_stop() -> &'static str {
    "Task Manager stopped!"
}