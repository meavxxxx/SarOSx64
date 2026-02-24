# SarOS — 64-битное монолитное ядро на Rust

Настоящее ядро операционной системы для x86_64.  
Архитектура вдохновлена Linux/FreeBSD. Boot protocol — Limine v8.

---

## Структура проекта

```
kernel/
├── Cargo.toml
├── Cargo.lock
├── build.rs                           — rustc-link-arg для linker script
├── kernel.ld                          — Linker script (high-half kernel, -2GiB)
│
├── .cargo/
│   └── config.toml                    — target = x86_64-unknown-none, rustflags
│
└── src/
    ├── main.rs                        — kernel_main(), Limine requests, panic handler
    │
    ├── arch.rs                        — pub mod x86_64
    ├── arch/
    │   ├── x86_64.rs                  — init_bsp(), udelay(), CPU feature setup
    │   └── x86_64/
    │       ├── limine.rs              — Limine v8 protocol (memmap, HHDM, FB, SMP)
    │       ├── gdt.rs                 — GDT + TSS (Ring 0/3, per-CPU)
    │       ├── idt.rs                 — IDT, ISR stubs, dispatch всех исключений
    │       ├── pic.rs                 — PIC 8259A (remap IRQ→32+, EOI, spurious)
    │       ├── io.rs                  — outb/inb, MSR rdmsr/wrmsr, CR0/CR3/CR4
    │       ├── timer.rs               — PIT 8254 @ 1000 Hz, TSC калибровка
    │       └── syscall_entry.rs       — SYSCALL/SYSRET entry (naked asm, swapgs)
    │
    ├── mm.rs                          — pub mod pmm/vmm/heap
    ├── mm/
    │   ├── pmm.rs                     — Buddy Allocator (4K–16M, из Limine mmap)
    │   ├── vmm.rs                     — 4-level paging, VMA, demand paging, CoW
    │   └── heap.rs                    — Slab Allocator → GlobalAlloc (Box/Vec/Arc)
    │
    ├── proc.rs                        — Process/PCB, Round-Robin scheduler,
    │                                    context_switch (naked asm), sleep/wake
    ├── proc/
    │   ├── elf.rs                     — ELF64 loader (ET_EXEC + ET_DYN/PIE)
    │   ├── exec.rs                    — execve(): загрузка ELF + прыжок в Ring 3
    │   ├── fork.rs                    — fork() CoW клонирование + waitpid()
    │   └── stack.rs                   — Userspace стек: argc/argv/envp/auxv (SysV ABI)
    │
    ├── drivers.rs                     — pub mod serial/vga/keyboard/logger
    ├── drivers/
    │   ├── serial.rs                  — UART 16550 / COM1 @ 115200 baud
    │   ├── vga.rs                     — Linear Framebuffer (Limine), bitmap шрифт 8×16
    │   ├── keyboard.rs                — PS/2, scancode Set 1, ring buffer, модификаторы
    │   ├── logger.rs                  — log::Log → serial + VGA
    │   └── font8x16.bin               — PSF1 bitmap шрифт (см. ниже как получить)
    │
    ├── sync.rs                        — pub mod spinlock
    ├── sync/
    │   └── spinlock.rs                — SpinLock (IF-aware) + RwSpinLock
    │
    └── syscall.rs                     — Таблица syscall, Linux x86_64 ABI
```

### font8x16.bin

```bash
# Нулевая заглушка (для компиляции без шрифта):
dd if=/dev/zero of=src/drivers/font8x16.bin bs=4096 count=1

# Реальный шрифт из Linux:
dd if=/usr/share/kbd/consolefonts/default8x16.psf \
   of=src/drivers/font8x16.bin bs=1 skip=4
```

---

## Реализованные компоненты

### Загрузка
- **Limine v8 protocol** — ядро объявляет статические request-структуры с magic numbers; загрузчик находит их в бинаре, заполняет memory map, HHDM offset, framebuffer, SMP info

### CPU / Архитектура
- **GDT** — null + kernel code/data (Ring 0) + user code/data (Ring 3) + TSS
- **TSS** — RSP0 (kernel stack при прерывании из Ring 3), IST #1 для double fault
- **IDT** — все 20 архитектурных исключений (`#DE #DB NMI #BP #OF #BR #UD #NM #DF #TS #NP #SS #GP #PF #MF #AC #MC #XM`), IRQ 0–15, int 0x80
- **PIC 8259A** — реинициализация, ремаппинг IRQ→INT 32–47, spurious IRQ detection
- **SYSCALL/SYSRET** — MSR STAR/LSTAR/SFMASK, `swapgs`, переключение kernel stack
- **CPU hardening** — WP, SMEP, SMAP, NXE, PGE

### Память
- **PMM** — Buddy Allocator, порядки 0–12 (4KiB – 16MiB), источник — Limine mmap
- **VMM** — 4-level paging (PML4→PDPT→PD→PT), Large Pages (2MiB), demand paging, Copy-on-Write, VMA list (аналог `vm_area_struct`)
- **Heap** — Slab Allocator (размеры 8–2048 байт) + PMM fallback; реализует `GlobalAlloc` → работают `Box`, `Vec`, `Arc`, `String`

### Процессы
- **PCB** — pid, ppid, state, cpu_context, address_space, vm, kernel_stack, signals
- **Scheduler** — Round-Robin с приоритетами, preemption через IRQ0
- **Context Switch** — naked asm, сохранение callee-saved, переключение CR3, обновление TSS.RSP0
- **ELF Loader** — `ET_EXEC` и `ET_DYN` (PIE), `PT_LOAD` сегменты, BSS zeroing, `PT_INTERP` для ld.so
- **fork()** — CoW клонирование PML4→PT, снятие `PTE_WRITABLE` у обоих, `waitpid()`
- **execve()** — новое адресное пространство, загрузка ELF, построение стека, прыжок в Ring 3 через `iretq`
- **User Stack** — System V AMD64 ABI: `argc / argv[] / envp[] / auxv` (AT_PHDR, AT_RANDOM, AT_EXECFN и др.)

### Системные вызовы (Linux x86_64 ABI)

| №   | Syscall          | Реализация                          |
|-----|------------------|-------------------------------------|
| 0   | `read`           | stdin (keyboard ring buffer)        |
| 1   | `write`          | stdout/stderr → serial + VGA        |
| 9   | `mmap`           | anonymous mapping, demand paging    |
| 11  | `munmap`         | unmap + освобождение VMA            |
| 12  | `brk`            | heap expansion                      |
| 39  | `getpid`         | текущий PID                         |
| 57  | `fork`           | CoW клонирование                    |
| 59  | `execve`         | загрузка ELF + Ring 3               |
| 60  | `exit`           | завершение → Zombie                 |
| 61  | `wait4`          | ожидание дочернего процесса         |
| 63  | `uname`          | системная информация                |
| 110 | `getppid`        | родительский PID                    |
| 228 | `clock_gettime`  | монотонное время (TSC)              |
| 231 | `exit_group`     | = exit                              |

### Драйверы
- **UART 16550** — COM1, 115200 baud, вывод до инициализации VGA
- **Framebuffer** — linear FB от Limine, bitmap шрифт 8×16, прокрутка экрана
- **PS/2 Keyboard** — IRQ1, scancode Set 1, модификаторы (Shift/Ctrl/Alt/Caps), ring buffer

---

## Сборка

### Требования

```bash
# Rust nightly (нужны: naked_functions, abi_x86_interrupt, alloc_error_handler)
rustup install nightly
rustup default nightly
rustup component add rust-src llvm-tools-preview

# Системные утилиты
sudo apt install xorriso qemu-system-x86   # Debian/Ubuntu
brew install xorriso qemu                   # macOS
```

### Компиляция

```bash
cargo build --release
# → target/x86_64-unknown-none/release/kernel
```

---

## Запуск в QEMU

### 1. Скачать Limine

```bash
git clone https://github.com/limine-bootloader/limine.git \
    --branch=v8.x-binary --depth=1
```

### 2. Создать ISO

```bash
KERNEL=target/x86_64-unknown-none/release/kernel

mkdir -p iso/boot/limine
cp $KERNEL iso/boot/kernel
cp limine/limine-bios.sys \
   limine/limine-bios-cd.bin \
   limine/limine-uefi-cd.bin \
   iso/boot/limine/

cat > iso/boot/limine/limine.cfg << 'EOF'
timeout: 0

/MyOS
    protocol: limine
    kernel_path: boot():/boot/kernel
EOF

xorriso -as mkisofs \
    -b boot/limine/limine-bios-cd.bin \
    -no-emul-boot -boot-load-size 4 -boot-info-table \
    --efi-boot boot/limine/limine-uefi-cd.bin -efi-boot-part \
    --efi-boot-image --protective-msdos-label \
    iso -o kernel.iso

./limine/limine bios-install kernel.iso
```

### 3. Запустить

```bash
qemu-system-x86_64 \
    -cdrom kernel.iso \
    -m 256M \
    -serial stdio \
    -no-reboot \
    -no-shutdown
```

### Полезные флаги QEMU

```bash
-enable-kvm          # аппаратная виртуализация (только Linux)
-smp 4               # 4 CPU ядра
-s -S                # GDB сервер на :1234, старт на паузе
-d int,cpu_reset     # лог прерываний
-D qemu.log          # куда писать лог
```

### GDB отладка

```bash
gdb target/x86_64-unknown-none/release/kernel
(gdb) target remote :1234
(gdb) break kernel_main
(gdb) continue
(gdb) x/10i $rip        # дизассемблировать текущую позицию
(gdb) info registers
```

---

## Адресное пространство

```
Виртуальное (48-bit canonical):
  0x0000_0000_0000_0000 — 0x0000_7FFF_FFFF_FFFF   User space  (128 TiB)
  0xFFFF_8000_0000_0000 — 0xFFFF_FFFF_FFFF_FFFF   Kernel space (128 TiB)

Kernel space:
  HHDM_OFFSET + [0 .. RAM]    — Прямой маппинг всей физ. памяти (Limine HHDM)
  0xFFFF_FFFF_8020_0000 — ... — Код и данные ядра (.text / .rodata / .data / .bss)

User space (один процесс):
  0x0000_5555_5555_0000       — PIE (ET_DYN) base
  0x0000_7FFF_0000_0000       — ld.so base
  0x0000_7FFF_F800_0000       — Stack top (растёт вниз, 8 MiB)

Физическая память:
  0x0000_0000 — 0x000F_FFFF   BIOS, VGA text buffer, зарезервировано
  0x0010_0000 — 0x001F_FFFF   BIOS reclaimable
  0x0020_0000 — ...           Ядро (загружается Limine)
  после ядра  — конец RAM     Свободные фреймы (Buddy Allocator)
```

---

## Что дальше

- [ ] **VFS + initrd (cpio)** — чтобы `execve` мог реально найти файл на диске
- [ ] **Сигналы POSIX** — `SIGKILL`, `SIGCHLD`, `SIGSEGV`, доставка через `sigreturn`
- [ ] **APIC** — Local APIC + I/O APIC вместо PIC 8259A
- [ ] **SMP** — per-CPU GDT/IDT/TSS, IPI, TLB shootdown, атомарный планировщик
- [ ] **Порт musl libc** — для запуска настоящих userspace программ
- [ ] **Сетевой стек** — virtio-net + минимальный TCP/IP
