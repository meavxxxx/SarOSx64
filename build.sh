#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")"

if [ ! -d limine ]; then
  git clone https://github.com/limine-bootloader/limine.git --branch=v8.x-binary --depth=1 limine
fi

cargo +nightly build --target x86_64-unknown-none

mkdir -p iso/boot/limine
cp target/x86_64-unknown-none/debug/SarOS iso/boot/kernel
cp limine/limine-bios.sys limine/limine-bios-cd.bin limine/limine-uefi-cd.bin iso/boot/limine/

cat > iso/boot/limine/limine.conf << 'EOF'
timeout: 0
default_entry: 1

/SarOS
protocol: limine
path: boot():/boot/kernel
EOF

xorriso -as mkisofs \
  -b boot/limine/limine-bios-cd.bin \
  -no-emul-boot -boot-load-size 4 -boot-info-table \
  --efi-boot boot/limine/limine-uefi-cd.bin -efi-boot-part \
  --efi-boot-image --protective-msdos-label \
  iso -o kernel.iso

./limine/limine.exe bios-install kernel.iso

echo "Done: kernel.iso built successfully"
