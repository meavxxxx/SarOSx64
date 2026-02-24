use crate::arch::x86_64::io::{inb, io_wait, outb};

const PIC1_CMD: u16 = 0x20;
const PIC1_DATA: u16 = 0x21;
const PIC2_CMD: u16 = 0xA0;
const PIC2_DATA: u16 = 0xA1;

const ICW1_ICW4: u8 = 0x01;
const ICW1_SINGLE: u8 = 0x02;
const ICW1_INIT: u8 = 0x10;
const ICW4_8086: u8 = 0x01;
const PIC_EOI: u8 = 0x20;
const PIC_READ_ISR: u8 = 0x0B;

pub const IRQ_BASE_MASTER: u8 = 32;
pub const IRQ_BASE_SLAVE: u8 = 40;

pub fn init() {
    unsafe {
        let mask1 = inb(PIC1_DATA);
        let mask2 = inb(PIC2_DATA);

        outb(PIC1_CMD, ICW1_INIT | ICW1_ICW4);
        io_wait();
        outb(PIC2_CMD, ICW1_INIT | ICW1_ICW4);
        io_wait();

        outb(PIC1_DATA, IRQ_BASE_MASTER);
        io_wait();
        outb(PIC2_DATA, IRQ_BASE_SLAVE);
        io_wait();

        outb(PIC1_DATA, 0b0000_0100);
        io_wait();
        outb(PIC2_DATA, 0b0000_0010);
        io_wait();

        outb(PIC1_DATA, ICW4_8086);
        io_wait();
        outb(PIC2_DATA, ICW4_8086);
        io_wait();

        outb(PIC1_DATA, 0b1111_1100);
        outb(PIC2_DATA, 0b1111_1111);
    }
}

pub fn send_eoi(irq: u8) {
    unsafe {
        if irq >= 8 {
            outb(PIC2_CMD, PIC_EOI);
        }
        outb(PIC1_CMD, PIC_EOI);
    }
}

pub fn send_eoi_master() {
    unsafe {
        outb(PIC1_CMD, PIC_EOI);
    }
}

pub fn mask_irq(irq: u8) {
    let (port, bit) = irq_to_port_bit(irq);
    unsafe {
        let val = inb(port);
        outb(port, val | (1 << bit));
    }
}

pub fn unmask_irq(irq: u8) {
    let (port, bit) = irq_to_port_bit(irq);
    unsafe {
        let val = inb(port);
        outb(port, val & !(1 << bit));
    }
}

fn irq_to_port_bit(irq: u8) -> (u16, u8) {
    if irq < 8 {
        (PIC1_DATA, irq)
    } else {
        (PIC2_DATA, irq - 8)
    }
}

pub fn is_spurious_irq7() -> bool {
    unsafe {
        outb(PIC1_CMD, PIC_READ_ISR);
        inb(PIC1_CMD) & 0x80 == 0
    }
}

pub fn is_spurious_irq15() -> bool {
    unsafe {
        outb(PIC2_CMD, PIC_READ_ISR);
        inb(PIC2_CMD) & 0x80 == 0
    }
}

pub fn disable() {
    unsafe {
        outb(PIC1_DATA, 0xFF);
        outb(PIC2_DATA, 0xFF);
    }
}
