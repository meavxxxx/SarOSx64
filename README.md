# SarOSx64

Экспериментальное x86_64 ядро на Rust (`no_std`), загружается через Limine (BIOS/UEFI).

## Что реализовано

### Архитектура / загрузка
- Загрузка через Limine (BIOS)
- GDT с TSS для переключения стека ядра
- IDT с обработчиками исключений и прерываний
- 8259 PIC (реремаппинг IRQ)
- PIT (IRQ0) — планировщик тиков; TSC — высокоточное время (`uptime_ms`)
- SYSCALL/SYSRET (MSR setup + entry stub)

### Память
- PMM: buddy-аллокатор (order 0–12), управление физическими фреймами
- VMM: 4-уровневые страничные таблицы (PML4), 4KiB/2MiB страницы, demand paging, CoW, VMA
- Heap: slab-аллокатор (8–2048 байт) + multi-page аллокации через PMM

### Процессы / планировщик
- `Process` с CpuContext, AddressSpace, VmSpace, приоритетом и тайм-слайсом
- Кооперативно-вытесняющий round-robin по приоритету
- Переключение контекста на naked assembly (callee-saved + RSP/RIP/RFLAGS)
- `RUN_QUEUE: SpinLock<RunQueue>` — глобальное состояние планировщика

### Файловая система
- VFS trait-слой: `Inode`, `File`, `FileType`, `Stat`, `Errno`
- ramfs: in-memory ФС
- Rootfs монтируется при старте (`/bin`, `/etc`, `/tmp`, `/home`, `/dev`, `/proc`, `/images`)
- Резолюция путей, поддержка симлинков

### Драйверы
| Драйвер | Описание |
|---|---|
| `serial.rs` | COM1 UART, `serial_print!`/`serial_println!` |
| `vga.rs` | Framebuffer, шрифт 8×16, скроллинг, цвета, `draw_bitmap()` |
| `keyboard.rs` | PS/2 клавиатура |
| `logger.rs` | Мост `log` крейта → serial |
| `bmp.rs` | Декодер 24-bit uncompressed BMP |
| `pci.rs` | Перечисление PCI bus (порты 0xCF8/0xCFC), BAR, IRQ |
| `ide.rs` | ATA PIO LBA28/LBA48, master/slave, primary/secondary |
| `fat32.rs` | Read-only FAT32, LFN support, MBR partition detection |
| `mbr.rs` | MBR partition table reader |

### Shell
Встроенные команды: `help`, `ls`, `cd`, `pwd`, `cat`, `echo`, `mkdir`, `touch`, `rm`, `rmdir`,
`mv`, `cp`, `write`, `stat`, `ln`, `view`, `lspci`, `drives`, `mount`, `umount`,
`clear`, `history`, `uname`, `uptime`, `free`, `reboot`, `halt`

### Syscall
Linux-совместимые номера. Обрабатываются: `read`/`write`, `fork`/`vfork`, `execve`, `exit`,
`waitpid`, `getpid`/`getppid`/`gettid`, `getuid`/`getgid`, `mmap`/`munmap`/`brk`,
`uname`, `clock_gettime`. Поддерживается как `SYSCALL`, так и `int 0x80`.

---

## Требования

- `rustup` + nightly toolchain
- `qemu-system-x86_64`
- `xorriso`
- `git`

## Сборка ядра

```bash
cargo +nightly build --target x86_64-unknown-none
```

## Сборка ISO

```bash
bash build.sh
```

Скрипт клонирует Limine (если нет), компилирует ядро, собирает ISO. Результат: `kernel.iso`.

## Запуск (только serial)

```bash
qemu-system-x86_64 -cdrom kernel.iso -m 512M -serial stdio -no-reboot -no-shutdown
```

## Запуск с диском

```bash
qemu-img create -f raw disk.img 512M
qemu-system-x86_64 -cdrom kernel.iso -hda disk.img -m 512M -serial stdio -no-reboot -no-shutdown -boot d
```

Диск виден в shell как `hda` (команда `drives`).

## Примечание

Весь вывод идёт в serial-консоль (`-serial stdio`). Framebuffer/VGA — вторичный вывод;
чёрное окно QEMU при `-serial stdio` является нормальным поведением.

---

## Roadmap

- [x] PCI bus enumeration
- [x] ATA/IDE PIO driver (LBA28/LBA48)
- [x] MBR partition table reader
- [x] FAT32 read-only filesystem (LFN support, VFS-integrated)
- [x] ELF64 loader + ring 3 user-space (PIE/static, aux vectors, CoW fork)
- [ ] ELF loader
- [ ] Ring 3 user-space + изоляция памяти
- [ ] Минимальная libc / musl
- [ ] virtio-net / e1000 сеть
- [ ] TCP/IP стек
