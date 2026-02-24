use crate::arch::x86_64::idt::InterruptFrame;
use crate::arch::x86_64::io::inb;
use crate::sync::spinlock::SpinLock;

const KB_DATA: u16 = 0x60;
const KB_STATUS: u16 = 0x64;

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

pub fn read_char_blocking() -> u8 {
    loop {
        if let Some(c) = read_char() {
            return c;
        }
        crate::proc::scheduler::sleep_current();
    }
}
