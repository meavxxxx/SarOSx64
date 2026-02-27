use crate::arch::x86_64::idt::InterruptFrame;
use crate::arch::x86_64::io::{inb, outb};
use crate::sync::spinlock::SpinLock;

const KB_DATA: u16 = 0x60;
const KB_STATUS: u16 = 0x64;

/// Wait until the i8042 input buffer is empty (bit 1 of status port).
/// Must be called before writing a command or data byte to the controller.
fn i8042_wait_write() {
    unsafe {
        while inb(KB_STATUS) & 0x02 != 0 {
            core::hint::spin_loop();
        }
    }
}

/// Wait until the i8042 output buffer has data ready (bit 0 of status port).
fn i8042_wait_read() {
    unsafe {
        while inb(KB_STATUS) & 0x01 == 0 {
            core::hint::spin_loop();
        }
    }
}

/// Initialize the i8042 PS/2 controller:
/// 1. Disable keyboard port (0xAD) to stop clock during init
/// 2. Flush output buffer
/// 3. Read CCB, set KIE=1, clear clock-disable bit, write back
/// 4. Re-enable keyboard port (0xAE)
/// 5. Flush again to discard any startup bytes from the keyboard device
pub fn init() {
    unsafe {
        // Disable first PS/2 port so keystrokes don't arrive during init.
        i8042_wait_write();
        outb(KB_STATUS, 0xAD);

        // Flush any stale bytes from the i8042 output buffer.
        while inb(KB_STATUS) & 0x01 != 0 {
            let _ = inb(KB_DATA);
        }

        // Command 0x20 = "Read CCB"; result arrives at port 0x60.
        i8042_wait_write();
        outb(KB_STATUS, 0x20);
        i8042_wait_read();
        let ccb = inb(KB_DATA);
        crate::serial_println!("[KB] i8042 CCB = {:#04x}", ccb);

        // Bit 0 = Keyboard Interrupt Enable (KIE).
        // Bit 4 = Keyboard Clock Disable — clear it so the keyboard is enabled.
        let new_ccb = (ccb | 0x01) & !0x10;

        // Command 0x60 = "Write CCB"; follow with the new byte on port 0x60.
        i8042_wait_write();
        outb(KB_STATUS, 0x60);
        i8042_wait_write();
        outb(KB_DATA, new_ccb);
        crate::serial_println!("[KB] i8042 CCB → {:#04x} (KIE=1)", new_ccb);

        // Re-enable first PS/2 port — this is the critical step that lets the
        // keyboard generate IRQ1 for keystrokes.
        i8042_wait_write();
        outb(KB_STATUS, 0xAE);

        // Flush any startup/init bytes the keyboard device may have sent
        // (e.g. power-on self-test result 0xAA) to prevent spurious IRQs.
        while inb(KB_STATUS) & 0x01 != 0 {
            let _ = inb(KB_DATA);
        }
    }
}

static SCANCODE_MAP: &[u8] = &[
    0, 27, b'1', b'2', b'3', b'4', b'5', b'6', b'7', b'8', b'9', b'0', b'-', b'=', 8, 9, b'q',
    b'w', b'e', b'r', b't', b'y', b'u', b'i', b'o', b'p', b'[', b']', b'\n', 0, b'a', b's', b'd',
    b'f', b'g', b'h', b'j', b'k', b'l', b';', b'\'', b'`', 0, b'\\', b'z', b'x', b'c', b'v', b'b',
    b'n', b'm', b',', b'.', b'/', 0, b'*', 0, b' ', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];

const KB_BUF_SIZE: usize = 256;

struct KbBuffer {
    buf: [u8; KB_BUF_SIZE],
    head: usize,
    tail: usize,
}

impl KbBuffer {
    const fn new() -> Self {
        Self {
            buf: [0; KB_BUF_SIZE],
            head: 0,
            tail: 0,
        }
    }

    fn push(&mut self, c: u8) {
        let next = (self.head + 1) % KB_BUF_SIZE;
        if next != self.tail {
            self.buf[self.head] = c;
            self.head = next;
        }
    }

    fn pop(&mut self) -> Option<u8> {
        if self.head == self.tail {
            return None;
        }
        let c = self.buf[self.tail];
        self.tail = (self.tail + 1) % KB_BUF_SIZE;
        Some(c)
    }

    fn is_empty(&self) -> bool {
        self.head == self.tail
    }
}

static KB_BUF: SpinLock<KbBuffer> = SpinLock::new(KbBuffer::new());

struct Modifiers {
    shift: bool,
    ctrl: bool,
    alt: bool,
    caps: bool,
}

static MODS: SpinLock<Modifiers> = SpinLock::new(Modifiers {
    shift: false,
    ctrl: false,
    alt: false,
    caps: false,
});

const SC_LSHIFT: u8 = 0x2A;
const SC_RSHIFT: u8 = 0x36;
const SC_LCTRL: u8 = 0x1D;
const SC_LALT: u8 = 0x38;
const SC_CAPS: u8 = 0x3A;
const SC_BREAK: u8 = 0x80;

pub fn irq_keyboard(_frame: &mut InterruptFrame) {
    let sc = unsafe { inb(KB_DATA) };
    crate::serial_println!("[KB] sc={:#04x}", sc);
    process_scancode(sc);
}

fn process_scancode(sc: u8) {
    let released = sc & SC_BREAK != 0;
    let sc_clean = sc & !SC_BREAK;

    let mut mods = MODS.lock();

    match sc_clean {
        SC_LSHIFT | SC_RSHIFT => {
            mods.shift = !released;
            return;
        }
        SC_LCTRL => {
            mods.ctrl = !released;
            return;
        }
        SC_LALT => {
            mods.alt = !released;
            return;
        }
        SC_CAPS if !released => {
            mods.caps = !mods.caps;
            return;
        }
        _ => {}
    }

    if released {
        return;
    }

    if (sc_clean as usize) < SCANCODE_MAP.len() {
        let mut c = SCANCODE_MAP[sc_clean as usize];
        if c == 0 {
            return;
        }

        if c.is_ascii_alphabetic() {
            let upper = mods.shift ^ mods.caps;
            if upper {
                c = c.to_ascii_uppercase();
            }
        } else if mods.shift {
            c = shifted_char(c);
        }

        if mods.ctrl && c.is_ascii_alphabetic() {
            c = c.to_ascii_uppercase() - b'@';
        }

        drop(mods);
        KB_BUF.lock().push(c);
        crate::proc::wake_up_all_sleeping();
    }
}

fn shifted_char(c: u8) -> u8 {
    match c {
        b'1' => b'!',
        b'2' => b'@',
        b'3' => b'#',
        b'4' => b'$',
        b'5' => b'%',
        b'6' => b'^',
        b'7' => b'&',
        b'8' => b'*',
        b'9' => b'(',
        b'0' => b')',
        b'-' => b'_',
        b'=' => b'+',
        b'[' => b'{',
        b']' => b'}',
        b'\\' => b'|',
        b';' => b':',
        b'\'' => b'"',
        b'`' => b'~',
        b',' => b'<',
        b'.' => b'>',
        b'/' => b'?',
        c => c,
    }
}

pub fn read_char() -> Option<u8> {
    KB_BUF.lock().pop()
}

pub fn push_char(c: u8) {
    KB_BUF.lock().push(c);
    crate::proc::wake_up_all_sleeping();
}

/// Race-free blocking read: holds IF=0 across the check-and-sleep transition
/// so a keyboard IRQ cannot arrive after the buffer check but before the
/// process is marked Sleeping (which would leave it asleep with data pending).
pub fn wait_key() -> u8 {
    use crate::arch::x86_64::io::{cli, sti, RFLAGS_IF};
    loop {
        let rflags = unsafe { cli() };
        if let Some(c) = read_char() {
            if rflags & RFLAGS_IF != 0 {
                sti();
            }
            return c;
        }
        crate::proc::scheduler::sleep_current();
    }
}
