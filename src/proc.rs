pub mod elf;
pub mod exec;
pub mod fork;
pub mod stack;

use crate::mm::vmm::{AddressSpace, VmSpace};
use crate::sync::spinlock::SpinLock;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};

static NEXT_PID: AtomicU32 = AtomicU32::new(1);
pub fn alloc_pid() -> u32 {
    NEXT_PID.fetch_add(1, Ordering::Relaxed)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ProcessState {
    Running,
    Runnable,
    Sleeping,
    Zombie,
    Dead,
}

#[derive(Debug, Default, Clone, Copy)]
#[repr(C)]
pub struct CpuContext {
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub rbp: u64,
    pub rbx: u64,
    pub rsp: u64,
    pub rip: u64,
    pub rflags: u64,
    pub cs: u64,
    pub ss: u64,
}

pub struct Process {
    pub pid: u32,
    pub ppid: u32,
    pub state: ProcessState,
    pub context: CpuContext,
    pub address_space: AddressSpace,
    pub vm: VmSpace,
    pub kernel_stack: u64,
    pub kernel_stack_size: usize,
    pub priority: u8,
    pub time_slice: u32,
    pub base_slice: u32,
    pub exit_code: i32,
    pub name: [u8; 32],
    pub pending_signals: u64,
    pub signal_mask: u64,
}

impl Process {
    pub const KERNEL_STACK_SIZE: usize = 16 * 1024;
    pub const DEFAULT_TIME_SLICE: u32 = 10;

    pub fn new_kernel(name: &str, entry: fn() -> !, priority: u8) -> Option<Arc<SpinLock<Self>>> {
        use crate::arch::x86_64::limine::phys_to_virt;
        use crate::mm::pmm::alloc_frames;
        let pid = alloc_pid();
        let stack_phys = alloc_frames(2)?;
        let stack_virt = phys_to_virt(stack_phys);
        let stack_top = stack_virt + Self::KERNEL_STACK_SIZE as u64;
        let ctx = CpuContext {
            rip: entry as u64,
            rsp: stack_top,
            rflags: 0x0202,
            cs: crate::arch::x86_64::gdt::SEG_KERNEL_CODE as u64,
            ss: crate::arch::x86_64::gdt::SEG_KERNEL_DATA as u64,
            ..Default::default()
        };
        let mut name_bytes = [0u8; 32];
        let n = name.len().min(31);
        name_bytes[..n].copy_from_slice(&name.as_bytes()[..n]);
        Some(Arc::new(SpinLock::new(Self {
            pid,
            ppid: 0,
            state: ProcessState::Runnable,
            context: ctx,
            address_space: AddressSpace::kernel(),
            vm: VmSpace::new(),
            kernel_stack: stack_virt,
            kernel_stack_size: Self::KERNEL_STACK_SIZE,
            priority,
            time_slice: Self::DEFAULT_TIME_SLICE,
            base_slice: Self::DEFAULT_TIME_SLICE,
            exit_code: 0,
            name: name_bytes,
            pending_signals: 0,
            signal_mask: 0,
        })))
    }

    pub fn name_str(&self) -> &str {
        let end = self.name.iter().position(|&b| b == 0).unwrap_or(32);
        core::str::from_utf8(&self.name[..end]).unwrap_or("???")
    }
}

pub struct RunQueue {
    pub queue: Vec<Arc<SpinLock<Process>>>,
    pub current: Option<Arc<SpinLock<Process>>>,
}

impl RunQueue {
    const fn new() -> Self {
        Self {
            queue: Vec::new(),
            current: None,
        }
    }
    fn pick_next(&mut self) -> Option<Arc<SpinLock<Process>>> {
        let best = self
            .queue
            .iter()
            .enumerate()
            .filter(|(_, p)| p.lock().state == ProcessState::Runnable)
            .min_by_key(|(_, p)| p.lock().priority)
            .map(|(i, _)| i);
        best.map(|i| self.queue.remove(i))
    }
}

pub static RUN_QUEUE: SpinLock<RunQueue> = SpinLock::new(RunQueue::new());

pub fn spawn(proc: Arc<SpinLock<Process>>) {
    RUN_QUEUE.lock().queue.push(proc);
}

pub fn current_process() -> Option<Arc<SpinLock<Process>>> {
    RUN_QUEUE.lock().current.clone()
}

pub fn tick() {
    let preempt = {
        let mut rq = RUN_QUEUE.lock();
        if let Some(ref c) = rq.current {
            let mut p = c.lock();
            if p.time_slice > 0 {
                p.time_slice -= 1;
            }
            p.time_slice == 0
        } else {
            false
        }
    };
    if preempt {
        schedule();
    }
}

pub fn schedule() {
    let mut rq = RUN_QUEUE.lock();
    let old = rq.current.take();
    if let Some(ref p) = old {
        let mut proc = p.lock();
        if proc.state == ProcessState::Running {
            proc.state = ProcessState::Runnable;
            proc.time_slice = proc.base_slice;
        }
        drop(proc);
        rq.queue.push(p.clone());
    }
    let next = rq.pick_next();
    if let Some(ref p) = next {
        p.lock().state = ProcessState::Running;
    }
    let next_for_switch = next.clone();
    rq.current = next.clone();
    drop(rq);

    if let (Some(old_a), Some(new_a)) = (old, next_for_switch) {
        if Arc::ptr_eq(&old_a, &new_a) {
            return;
        }
        unsafe {
            let kst = new_a.lock().kernel_stack + Process::KERNEL_STACK_SIZE as u64;
            crate::arch::x86_64::gdt::set_kernel_stack(kst);
            {
                let op = old_a.lock();
                let np = new_a.lock();
                if op.address_space.pml4_phys != np.address_space.pml4_phys {
                    np.address_space.activate();
                }
            }
            let oc = &mut old_a.lock().context as *mut CpuContext;
            let nc = &new_a.lock().context as *const CpuContext;
            context_switch(oc, nc);
        }
    } else if let Some(a) = next {
        unsafe {
            jump_to_context(&a.lock().context as *const CpuContext);
        }
    }
}

pub fn sleep_current() {
    if let Some(ref p) = RUN_QUEUE.lock().current {
        p.lock().state = ProcessState::Sleeping;
    }
    schedule();
}

pub fn wake_up(pid: u32) {
    let rq = RUN_QUEUE.lock();
    for p in &rq.queue {
        let mut proc = p.lock();
        if proc.pid == pid && proc.state == ProcessState::Sleeping {
            proc.state = ProcessState::Runnable;
            return;
        }
    }
}

#[unsafe(naked)]
pub unsafe extern "C" fn context_switch(old: *mut CpuContext, new: *const CpuContext) {
    core::arch::naked_asm!(
        "mov %rbx,40(%rdi)",
        "mov %rbp,32(%rdi)",
        "mov %r12,24(%rdi)",
        "mov %r13,16(%rdi)",
        "mov %r14, 8(%rdi)",
        "mov %r15, 0(%rdi)",
        "mov %rsp,48(%rdi)",
        "lea 1f(%rip),%rax",
        "mov %rax,56(%rdi)",
        "pushfq",
        "pop %rax",
        "mov %rax,64(%rdi)",
        "mov  0(%rsi),%r15",
        "mov  8(%rsi),%r14",
        "mov 16(%rsi),%r13",
        "mov 24(%rsi),%r12",
        "mov 32(%rsi),%rbp",
        "mov 40(%rsi),%rbx",
        "mov 48(%rsi),%rsp",
        "mov 64(%rsi),%rax",
        "push %rax",
        "popfq",
        "jmp *56(%rsi)",
        "1:",
        "ret",
        options(att_syntax)
    );
}

#[unsafe(naked)]
unsafe extern "C" fn jump_to_context(ctx: *const CpuContext) {
    core::arch::naked_asm!(
        "mov  0(%rdi),%r15",
        "mov  8(%rdi),%r14",
        "mov 16(%rdi),%r13",
        "mov 24(%rdi),%r12",
        "mov 32(%rdi),%rbp",
        "mov 40(%rdi),%rbx",
        "mov 48(%rdi),%rsp",
        "mov 64(%rdi),%rax",
        "push %rax",
        "popfq",
        "jmp *56(%rdi)",
        options(att_syntax)
    );
}

pub mod scheduler {
    pub use super::{current_process, schedule, sleep_current, spawn, tick, wake_up, RUN_QUEUE};
}
