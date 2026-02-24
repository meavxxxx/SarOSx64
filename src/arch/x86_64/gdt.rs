use core::mem;

pub const SEG_KERNEL_CODE: u16 = 0x08;
pub const SEG_KERNEL_DATA: u16 = 0x10;
pub const SEG_USER_DATA: u16 = 0x18 | 3;
pub const SEG_USER_CODE: u16 = 0x20 | 3;
pub const SEG_TSS: u16 = 0x28;

#[derive(Debug, Clone, Copy)]
#[repr(C, packed(4))]
pub struct Tss {
    _reserved0: u32,

    pub rsp: [u64; 3],
    _reserved1: u64,

    pub ist: [u64; 7],
    _reserved2: u64,
    _reserved3: u16,

    pub iopb: u16,
}

impl Tss {
    pub const fn new() -> Self {
        Self {
            _reserved0: 0,
            rsp: [0; 3],
            _reserved1: 0,
            ist: [0; 7],
            _reserved2: 0,
            _reserved3: 0,
            iopb: mem::size_of::<Tss>() as u16,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
struct SegDesc(u64);

impl SegDesc {
    const NULL: Self = Self(0);

    const fn new(access: u8, flags: u8) -> Self {
        let limit_low = 0xFFFFu64;
        let base_low = 0u64;
        let base_mid = 0u64;
        let limit_high = 0xFu64;
        let base_high = 0u64;

        let raw = limit_low
            | (base_low << 16)
            | (base_mid << 32)
            | ((access as u64) << 40)
            | (limit_high << 48)
            | ((flags as u64) << 52)
            | (base_high << 56);
        Self(raw)
    }
}

#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
struct TssDesc {
    low: u64,
    high: u64,
}

impl TssDesc {
    fn new(tss: &Tss) -> Self {
        let base = tss as *const Tss as u64;
        let limit = (mem::size_of::<Tss>() - 1) as u64;

        let low = (limit & 0xFFFF)
            | ((base & 0xFF_FFFF) << 16)
            | (0x89u64 << 40)
            | (((limit >> 16) & 0xF) << 48)
            | (((base >> 24) & 0xFF) << 56);

        let high = (base >> 32) & 0xFFFF_FFFF;

        Self { low, high }
    }
}

#[repr(C, align(16))]
struct Gdt {
    null: SegDesc,
    kcode: SegDesc,
    kdata: SegDesc,
    udata: SegDesc,
    ucode: SegDesc,
    tss: TssDesc,
}

#[repr(C, packed)]
struct Gdtr {
    limit: u16,
    base: u64,
}

pub struct CpuGdt {
    gdt: Gdt,
    pub tss: Tss,
}

impl CpuGdt {
    pub const fn new() -> Self {
        Self {
            gdt: Gdt {
                null: SegDesc::NULL,
                kcode: SegDesc::new(0x9a, 0x2),
                kdata: SegDesc::new(0x92, 0x0),
                udata: SegDesc::new(0xF2, 0x0),
                ucode: SegDesc::new(0xFA, 0x2),
                tss: TssDesc { low: 0, high: 0 },
            },
            tss: Tss::new(),
        }
    }

    pub fn set_kernel_stack(&mut self, rsp: u64) {
        self.tss.rsp[0] = rsp;
        self.gdt.tss = TssDesc::new(&self.tss);

        let gdtr = Gdtr {
            limit: (mem::size_of::<Gdt>() - 1) as u16,
            base: &self.gdt as *const Gdt as u64,
        };

        unsafe {
            core::arch::asm!(
                "lgdt ({gdtr})",

                "movw {kdata:x}, %ax",
                "movw %ax, %ds",
                "movw %ax, %es",
                "movw %ax, %ss",

                "xor %eax, %eax",
                "movw %ax, %fs",
                "movw %ax, %gs",

                "ltr {tss:x}",

                gdtr = in(reg) &gdtr,
                kdata = in(reg) SEG_KERNEL_DATA,
                tss = in(reg) SEG_TSS,
                options(att_syntax)
            );
        }
    }
}

const MAX_CPUS: usize = 256;

static mut CPU_GDTS: [CpuGdt; 1] = [CpuGdt::new()];

pub fn init_bsp(kernel_stack_top: u64) {
    unsafe {
        CPU_GDTS[0].set_kernel_stack(kernel_stack_top);
    }
}

pub fn current_tss() -> &'static mut Tss {
    unsafe { &mut CPU_GDTS[0].tss }
}

pub fn set_kernel_stack(rsp0: u64) {
    current_tss().rsp[0] = rsp0;
}
