# SarOSx64

Экспериментальное x86_64 ядро на Rust (`no_std`), загружается через Limine.

## Что уже работает

- Загрузка через Limine (BIOS)
- Инициализация:
  - GDT
  - IDT
  - PIC
  - PIT
  - SYSCALL/SYSRET
- Логирование в serial (`-serial stdio`)

## Требования

- `rustup` + nightly
- `qemu-system-x86_64`
- `xorriso`
- `git`

## Сборка ядра

```bash
cargo +nightly build --target x86_64-unknown-none
```

## Сборка ISO

```bash
KERNEL=target/x86_64-unknown-none/debug/SarOS
cp "$KERNEL" iso/boot/kernel

xorriso -as mkisofs \
  -b boot/limine/limine-bios-cd.bin \
  -no-emul-boot -boot-load-size 4 -boot-info-table \
  --efi-boot boot/limine/limine-uefi-cd.bin -efi-boot-part \
  --efi-boot-image --protective-msdos-label \
  iso -o kernel.iso

./limine/limine.exe bios-install kernel.iso
```

## Запуск

```bash
qemu-system-x86_64 -cdrom kernel.iso -m 512M -serial stdio -no-reboot -no-shutdown
```

## Примечание

Чёрное окно QEMU при этом может быть нормой: вывод идёт в serial-консоль, а не framebuffer.
