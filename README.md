<div align="center">

<img src="assets/lunaicon.jpg" width="160" alt="MiLuna">

# MiLuna 🌙

**Дистрибутив операционной системы для x86_64 на ядре [MiUx](https://github.com/mel0k1/MiLuna) 7.0.0**

Rust-ядро · оконный менеджер на C · SSE2 · GRUB 2 · браузер · сеть · FAT32

![CI](https://github.com/mel0k1/MiLuna/actions/workflows/build.yml/badge.svg)
![License](https://img.shields.io/badge/license-Apache--2.0-blue)
![Rust](https://img.shields.io/badge/rust-nightly-orange)
![Arch](https://img.shields.io/badge/arch-x86__64-red)
![Version](https://img.shields.io/badge/version-1.0.0-purple)

</div>

---

## О проекте

**MiLuna** — любительский дистрибутив ОС, построенный на базе ядра **MiUx 7.0.0** от команды MDEVS. Из коробки вы получаете полноценную live-систему с текстовым терминалом, графической оболочкой с оконным менеджером, собственным веб-браузером, сетевым стеком с HTTPS и установщиком на жёсткий диск.

Проект собирается одной командой `make` в гибридный ISO-образ (загружается и по Legacy BIOS, и по UEFI), а GitHub Actions автоматически собирает `MiLuna.iso` на каждый коммит и прикладывает его к релизам по тегам.

## Возможности

**Ядро и железо**
- Ядро на Rust (`no_std`), загрузка через GRUB 2 (multiboot2, VBE/VESA 1920x1080)
- SSE/SSE2 включаются в загрузчике: аппаратная FP-арифметика для браузерного движка и GL, быстрые `memcpy`/`memset`
- Реальные данные железа: память из memory map multiboot, uptime через TSC
- Линейный фреймбуфер, частичная перерисовка экрана (fastblit)
- Клавиатура, мышь (включая колесо), ATA, SATA, ATAPI, PCI
- Файловые системы: FAT32 (чтение и запись), EXT4 (просмотр), TAR initrd
- Пользователи: вход, `useradd`, `userdel`, `passwd`

**Многозадачность и Ring 3**
- Вытесняющий планировщик: PIT 50 Гц (IRQ0), полное сохранение контекста (регистры + FXSAVE/FXRSTOR), приоритеты 1–9 (round-robin внутри уровня)
- Пользовательский режим: TSS (RSP0), user-сегменты GDT, U/S-страницы; процессы исполняются в ring 3
- ELF-загрузчик: пользовательские программы — настоящие ELF64, сегменты мапятся с правами (text R+X, rodata R, data/stack RW, всё неисполняемое помечено NX-битом)
- Быстрый путь `syscall`/`sysret` через MSR STAR/LSTAR/SFMASK + классический `int 0x80` как резервный вход
- Системные вызовы: `write`, `yield`, `exit`, `fork`, `exec`, `nice`, `getpid` — fork копирует образ и страницы с правами, exec перезагружает ELF в тот же процесс
- NX-бит (EFER.NXE): переход на data-страницу роняет процесс с #PF — W^X для пользовательской памяти
- Изоляция отказов: исключение в процессе убивает только процесс — ядро и GUI продолжают работать
- SMP: парсинг ACPI MADT (RSDP→RSDT/XSDT), INIT-SIPI через Local APIC, AP-процессоры просыпаются в long mode через trampoline на 0x8000; планирование на всех ядрах
- Task stealing с IPI-механизмом: resched-IPI (вектор 0x31) будит простаивающие ядра, планировщик per-CPU (GS base), spin-блокировки SCHED/FRAME
- Per-process таблицы страниц: у каждого процесса свой PML4/PDPT/PDT/PT, переключение CR3, ядро отображено в каждом адресном пространстве
- Copy-on-write fork: страницы родителя и ребёнка разделяются read-only (бит 9 PTE), запись ловит #PF и выделяет приватную копию, refcount кадров
- Сигналы и waitpid: SIGTERM/SIGKILL/SIGINT завершают процесс (exit 128+N), SIGUSR1/2/SIGCHLD игнорируются, заблокированный waitpid будится при смерти ребёнка, зомби видны в `tasks`
- HPET: ACPI HPET-таблица (GAS → MMIO 0xFED00000), 64-битный монотонный счётчик; `millis()` больше не требует 2-секундной калибровки RTC/TSC (осталась как fallback), `cpuinfo` показывает базу/частоту/время
- Инвариант пути выхода (гонка порчи памяти под TCG): слот и адресное пространство освобождаются только под SCHED_LOCK, только когда задача не `cur` ни на одном ядре и только после ухода всех ядер с её CR3 — `make_zombie` лишь помечает (флаг collected), реальный reap делает первый безопасный вызов планировщика; `waitpid` сканирует детей под блокировкой (иначе потерянное пробуждение)
- Демо в терминале: `hello`, `hello fault`, `nx`, `fork`, `prio`, `exec`, `sig` (COW+сигналы+waitpid), `smp` (fork-воркеры на все ядра), `cow`, `wait`, `race` (стресс reap-гонки: 6 быстрых детей на инстанс), `tasks`, `cpuinfo` (per-core тики/IPI/steal)

**Графический интерфейс** (`gui /start --all`)
- Оконный менеджер на C: рамки, тени, свернуть/развернуть/закрыть, панель задач, меню «Пуск», часы
- Полноценный рабочий стол: обои (BMP), иконки приложений, папки и ярлыки, контекстное меню
- **Alt+Tab** — переключение окон, **Aero Snap** — перетащите окно к верхнему или боковому краю экрана
- Подсветка кнопок заголовка при наведении мыши, живые часы в панели задач (RTC)
- 4 темы: Aero, Basic, Classic, Ubuntu
- Раскладка EN/RU (`Alt+Shift`), рабочий стол с папками и ярлыками
- Экран паники ядра с диагнозом исключения (вектор, имя, CR2, RIP) вместо молчаливого зависания

**Приложения**
- Terminal, Notepad, Paint (сохранение в `.bmp`), File Manager
- Settings, Network, Browser, Task Manager, Calendar, Clock
- Cre Tool (конструктор приложений), GL (3D-шестерёнки), HTML Viewer, Support Center, BMP-просмотр

**Сеть и интернет**
- Intel e1000, Realtek RTL8139, RTL8169
- Стек TCP/IP на smoltcp: DHCP, DNS, TCP, UDP, ICMP
- HTTP и HTTPS (TLS 1.3), перенаправления, gzip, cookies
- Собственный браузерный движок: HTML, CSS (селекторы, flex, grid), картинки PNG/JPEG/BMP, формы

## Скриншоты

| | |
|---|---|
| ![fetch в терминале](docs/screenshots/terminal.jpg) | ![Рабочий стол](docs/screenshots/desktop.jpg) |
| ![Aero Snap — превью зоны](docs/screenshots/snap.jpg) | ![Развернутое окно](docs/screenshots/maximized.png) |
| ![Task Manager](docs/screenshots/task-manager.jpg) | ![Календарь](docs/screenshots/calendar.jpg) |
| ![PC Info](docs/screenshots/pc-info.jpg) | ![Settings](docs/screenshots/settings.jpg) |
| ![Ring 3 процессы](docs/screenshots/ring3-tasks.jpg) | ![Изоляция отказов](docs/screenshots/ring3-fault.jpg) |
| ![ELF, NX и fork](docs/screenshots/ring3-elf.jpg) | ![SMP — 4 CPU](docs/screenshots/smp-cpuinfo.jpg) |

Aero Snap: перетащите окно к верхнему краю — развернётся на весь экран, к левому или правому — займёт половину.

`task-manager.jpg`: окно Task Manager показывает настоящие задачи ядра: PID, состояние (Running/Ready/Dead), тики таймера и последнее сообщение процесса. `ring3-tasks.jpg`: процесс `hello` исполняется в ring 3 и отчитывается через `int 0x80` (`ring3 task tick 308`). `ring3-fault.jpg`: процесс с намеренным `#UD` изолирован ядром (Dead, код 262) — система продолжила работу. `ring3-elf.jpg`: таблица процессов с колонкой PRIO — `hello fault` изолирован (262), `nx-test` убит #PF за исполнение NX-страницы (270). `smp-cpuinfo.jpg`: `cpuinfo` — 4 CPU из ACPI MADT, все AP проснулись через INIT-SIPI. `smp-tasks.jpg`: колонка CPU в `tasks` — четыре CPU-bound задачи `smp` распределены планировщиком по ядрам (task stealing), `cow` — fork с copy-on-write, `wait` — блокирующий `wait()` с кодом выхода 42.

## fetch

Традиционный `fetch` с фирменной луной доступен прямо в терминале (uptime и разрешение экрана — настоящие):

```
.....................................
.....................................
.............*%%%%:.................. mel0k1@MiLuna
.........-%%%%%%:.................... -------------
.......=%%#.=%%.........=............ OS:      MiLuna 1.0.0
......%%%...%%=.......:%%-........... Kernel:  MiUx 7.0.0 (Rust)
.....%%#....%%.....=%%%%%%%%*........ Shell:   miluna-term
....*%%.....%%-.......+%%#...........
....%%-.....+%%.........+............ Distro:  mel0k1
....%%.......%%%................#....
....%%-.......%%%%............+%%.... Uptime:  42s
....*%%:....:%%%%%%%%=::::-%%%%%+....
.....%%%%%%%%%:.....#%%%%%%%.%%%..... Screen:  1920x1080
......%%%%%%%...............%%#......
.......-%%%%%=...........:%%%:.......
.........:%%%%%=.....=*%%%%..........
.............+%%%%%%%%%+.............
.....................................
.....................................
```

## Сборка

Нужны Linux, nightly Rust, NASM, GRUB и утилиты для ISO:

```bash
sudo apt update
sudo apt install -y build-essential nasm grub-pc-bin grub-efi-amd64-bin \
                    grub-efi-ia32-bin grub-common xorriso mtools dosfstools

curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env

make setup    # nightly, rust-src, target x86_64-unknown-none
make          # собирает MiLuna.iso
```

Или просто заберите готовый `MiLuna.iso` со страницы [Actions](https://github.com/mel0k1/MiLuna/actions) — CI собирает образ на каждый коммит. На тегах `v*` ISO автоматически прикладывается к релизу.

## Запуск

```bash
make run
```

Или вручную, с сетью:

```bash
qemu-system-x86_64 -cdrom MiLuna.iso -m 1024M -vga std -nic user,model=e1000
```

Нужно не меньше 512 МБ памяти. Можно взять `model=rtl8139`.

### Запуск с FAT32-диском

```bash
make fat32-disk   # создаёт fat32.img (100 МБ)
make run-fat32    # диск + ISO
```

### Меню GRUB

- **Boot MiLuna (LiveCD)** — запуск системы
- **Install MiLuna** — установщик на диск
- **Reboot**, **Shutdown**

## Установка на диск

1. Загрузитесь с ISO и выберите «Install MiLuna».
2. Выберите диск и подтвердите установку.
3. Установщик копирует ядро в папку `MiLuna`, ставит GRUB и создаёт `system.cfg`.
4. Загрузитесь с установленного диска.

## Команды терминала

Полный список: `help`.

| Группа | Команды |
|---|---|
| Система | `ver`, `fetch`, `uptime`, `mem`, `about`, `tech`, `kernel`, `pc --info` |
| Пользователи | `login`, `logout`, `whoami`, `users`, `useradd`, `userdel`, `passwd` |
| Файлы | `dir`, `ls`, `cd`, `cat`, `disks`, `mount`, `format`, `fs` |
| EXT4 | `ext4`, `ext4ls`, `ext4cat`, `ext4info` |
| Сеть | `ifconfig`, `nslookup`, `wget`, `net` |
| Браузер | `browser [адрес]` |
| Приложения | `call`, `list-apps`, `gui /start --all`, `notepad /start`, `gl /start` |
| Процессы | `hello` (ring 3 демо), `hello fault` (демо изоляции отказов), `tasks` (таблица задач) |
| Экран | `display /scr 1920x1080`, `display /scr default` |

## Структура проекта

```
MiLuna/
├── src/
│   ├── lib.rs          ядро: главный цикл, терминал, приложения
│   ├── boot.asm        загрузчик (long mode, таблицы страниц, GDT)
│   ├── task.rs         процессы, планировщик, syscalls int 0x80
│   ├── trap.rs         единая точка входа прерываний/исключений, контекст-свитч
│   ├── gdt.rs          GDT с user-сегментами и TSS (RSP0)
│   ├── pit.rs          системный таймер 50 Гц (IRQ0)
│   ├── idt.rs          таблица прерываний, экран паники
│   ├── user/hello.asm  демо-программа для ring 3 (nasm -> flat bin)
│   ├── ui/             оконный менеджер на C (wm.c, gfx.c, taskbar.c)
│   ├── cui.rs, wm.rs   мост Rust <-> C и логика окон
│   ├── graphics.rs, vesa.rs, fastblit.rs   графика
│   ├── fs/             fat32.rs, ext4.rs, tar.rs
│   ├── net/            e1000, rtl8139, rtl8169, wifi, http
│   ├── web/            движок браузера
│   ├── installer.rs    установщик
│   └── syscfg.rs       system.cfg: настройки
├── assets/             иконка MiLuna
├── initrd/             содержимое initrd
├── tools/gen_fonts.py  генератор шрифтов
└── Makefile, linker.ld, Cargo.toml
```

## Roadmap

**Готово**
- [x] Реальные значения `mem` из memory map multiboot, `uptime` через TSC
- [x] SSE/SSE2: включены в загрузчике (CR0/CR4) и target features
- [x] Экран паники ядра (вектор, имя исключения, CR2, RIP) вместо `hlt`
- [x] Полный рабочий стол: панель задач, меню «Пуск», иконки, обои
- [x] Alt+Tab и Aero Snap (верх/лево/право с превью зоны)
- [x] Вытесняющая многозадачность: PIT 50 Гц (IRQ0), контекст-свитч из прерывания, FXSAVE/FXRSTOR
- [x] Ring 3: TSS, user-сегменты GDT, U/S-страницы, изоляция отказов
- [x] ELF-загрузчик с per-page правами сегментов и NX-битом (EFER.NXE), стек и сегменты раздельно
- [x] Быстрый путь `syscall`/`sysret` (STAR/LSTAR/SFMASK), кадр на стеке задачи, `int 0x80` как резерв
- [x] `fork`/`exec`/`nice`/`getpid`, приоритеты планировщика 1–9
- [x] SMP: ACPI MADT, Local APIC, INIT-SIPI bringup, per-AP TSS и GDT, `cpuinfo`
- [x] Task Manager показывает реальные задачи ядра вместо заглушек
- [x] Планирование на всех ядрах: per-CPU области (GS base), task stealing, resched-IPI (0x31), spin-блокировки
- [x] Per-process адресные пространства (свой CR3 у процесса) и Copy-on-write fork (refcount + soft-W бит PTE)
- [x] Сигналы (SIGTERM/KILL/INT фатальны, USR1/2/CHLD игнорируются) и блокирующий `waitpid` со сбором зомби
- [x] Калибровка TSC по RTC-границе секунды вместо PC-спикера (2.2x дрейф под QEMU)

**Дальше**
- [ ] LAPIC-таймер на всех ядрах (в QEMU TCG EOI из обработчика прерывания теряется — нужен HPET/реальное железо)
- [x] Чинить гонку порчи памяти при работе задач на AP: reap/waitpid под SCHED_LOCK, слот освобождается только когда задача не `cur` ни на одном ядре и все ядра ушли с её CR3 (`race` стресс-демо)
- [ ] `waitpid` с конкретным PID, группы процессов
- [x] HPET: монотонный источник времени вместо калибровки RTC/TSC (busy-wait `sleep` и ACPI poweroff — отдельно)
- [ ] LAPIC-таймер с калибровкой TICR от HPET (в QEMU TCG EOI из обработчика теряется — HPET-прерывания на реальном железе)
- [ ] Больше окон одновременно (сейчас 8), damage-tracking во всём композиторе
- [ ] Постепенный отказ от `static mut` в пользу безопасных абстракций
- [ ] Сжатие встроенных BMP-ресурсов (сейчас 4+ МБ)
- [ ] Проверка TLS-сертификатов в HTTPS
- [ ] Драйверы конкретных Wi-Fi карт

## Кредиты

MiLuna построена на ядре **MiUx 7.0.0**, написанном **Mikhail** и командой **MDEVS**. Спасибо за отличную базу!

- Ядро MiUx и его автор: MDEVS
- Дистрибутив MiLuna: **mel0k1**

## Лицензия

[Apache-2.0](LICENSE). Права на исходное ядро MiUx принадлежат его авторам.
