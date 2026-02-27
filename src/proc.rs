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
        let stack_phys = alloc_frames(Self::KERNEL_STACK_SIZE / crate::mm::pmm::PAGE_SIZE)?;
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

    /// Spawn a new user-mode process from ELF binary data.
    /// The process starts in ring 3 via IRETQ on first scheduling.
    pub fn new_user(
        name: &str,
        elf_data: &[u8],
        argv: &[Vec<u8>],
        envp: &[Vec<u8>],
        priority: u8,
    ) -> Result<Arc<SpinLock<Self>>, &'static str> {
        use crate::arch::x86_64::gdt::{SEG_USER_CODE, SEG_USER_DATA};
        use crate::arch::x86_64::limine::phys_to_virt;
        use crate::mm::pmm::alloc_frames;

        let pid = alloc_pid();
        let ppid = crate::proc::scheduler::current_process()
            .map(|p| p.lock().pid)
            .unwrap_or(0);

        // Kernel stack for this process
        let kstack_phys = alloc_frames(Self::KERNEL_STACK_SIZE / crate::mm::pmm::PAGE_SIZE)
            .ok_or("OOM: kernel stack")?;
        let kstack_virt = phys_to_virt(kstack_phys);
        let kstack_top = kstack_virt + Self::KERNEL_STACK_SIZE as u64;

        // User address space + ELF
        let mut space = AddressSpace::new_user().ok_or("OOM: address space")?;
        let mut vm = VmSpace::new();

        let pie_base = if crate::proc::exec::is_pie(elf_data) {
            crate::proc::exec::PIE_BASE
        } else {
            0
        };
        let loaded = crate::proc::elf::load_elf(elf_data, &mut space, &mut vm, pie_base)
            .map_err(|_| "ELF load failed")?;
        if loaded.interp_path.is_some() {
            return Err("PT_INTERP unsupported in spawn path");
        }

        // User stack with aux vectors
        let argv_refs: Vec<&[u8]> = argv.iter().map(|v| v.as_slice()).collect();
        let envp_refs: Vec<&[u8]> = envp.iter().map(|v| v.as_slice()).collect();
        let ustack = crate::proc::stack::build_user_stack(
            &mut space,
            &mut vm,
            &loaded,
            0,
            &argv_refs,
            &envp_refs,
            name.as_bytes(),
        )
        .ok_or("user stack build failed")?;

        // Set up IRETQ frame on the kernel stack so the first
        // jump_to_context → iretq_trampoline transitions to ring 3.
        //
        // IRETQ pops (from low to high address):
        //   [RIP] [CS] [RFLAGS] [RSP] [SS]
        let frame = unsafe {
            let p = (kstack_top as *mut u64).sub(5);
            p.add(0).write(loaded.entry); // RIP
            p.add(1).write(SEG_USER_CODE as u64); // CS
            p.add(2).write(0x0202u64); // RFLAGS (IF=1)
            p.add(3).write(ustack.initial_rsp); // RSP
            p.add(4).write(SEG_USER_DATA as u64); // SS
            p as u64
        };

        let ctx = CpuContext {
            rip: iretq_trampoline as u64,
            rsp: frame,
            rflags: 0x0202,
            cs: crate::arch::x86_64::gdt::SEG_KERNEL_CODE as u64,
            ss: crate::arch::x86_64::gdt::SEG_KERNEL_DATA as u64,
            ..Default::default()
        };

        let mut name_bytes = [0u8; 32];
        let n = name.len().min(31);
        name_bytes[..n].copy_from_slice(&name.as_bytes()[..n]);

        Ok(Arc::new(SpinLock::new(Self {
            pid,
            ppid,
            state: ProcessState::Runnable,
            context: ctx,
            address_space: space,
            vm,
            kernel_stack: kstack_virt,
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

pub fn process_state(pid: u32) -> Option<ProcessState> {
    let rq = RUN_QUEUE.lock();
    if let Some(ref cur) = rq.current {
        let p = cur.lock();
        if p.pid == pid {
            return Some(p.state);
        }
    }
    for proc in &rq.queue {
        let p = proc.lock();
        if p.pid == pid {
            return Some(p.state);
        }
    }
    None
}

#[derive(Clone, Copy)]
pub struct ChildProcessInfo {
    pub pid: u32,
    pub state: ProcessState,
    pub name: [u8; 32],
}

pub fn children_of_current() -> Vec<ChildProcessInfo> {
    let rq = RUN_QUEUE.lock();
    let parent_pid = match rq.current.as_ref() {
        Some(cur) => cur.lock().pid,
        None => return Vec::new(),
    };

    let mut children = Vec::new();
    for proc in &rq.queue {
        let p = proc.lock();
        if p.ppid == parent_pid {
            children.push(ChildProcessInfo {
                pid: p.pid,
                state: p.state,
                name: p.name,
            });
        }
    }
    children
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
        schedule_from_irq();
    }
}

pub fn schedule() {
    schedule_impl(false);
}

fn schedule_from_irq() {
    schedule_impl(true);
}

fn schedule_impl(in_irq: bool) {
    let mut rq = RUN_QUEUE.lock();
    let old = rq.current.take();
    if let Some(ref p) = old {
        let mut proc = p.lock();
        if proc.state == ProcessState::Running {
            proc.state = ProcessState::Runnable;
            proc.time_slice = proc.base_slice;
        }
        // Keep zombies in the global queue until a parent reaps them via waitpid.
        let requeue = proc.state != ProcessState::Dead;
        drop(proc);
        if requeue {
            rq.queue.push(p.clone());
        }
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
            crate::arch::x86_64::syscall_entry::set_kernel_stack(kst);
            {
                let op = old_a.lock();
                let np = new_a.lock();
                if op.address_space.pml4_phys != np.address_space.pml4_phys {
                    np.address_space.activate();
                }
            }
            let oc = &mut old_a.lock().context as *mut CpuContext;
            let nc = &new_a.lock().context as *const CpuContext;
            if in_irq {
                context_switch_irq(oc, nc);
            } else {
                context_switch(oc, nc);
            }
        }
    } else if let Some(a) = next {
        unsafe {
            // Must drop the SpinGuard BEFORE jumping — jump_to_context never
            // returns, so a temporary created in the call expression would
            // never be dropped, leaking the process SpinLock forever.
            let ctx_ptr = {
                let g = a.lock();
                &g.context as *const CpuContext
            }; // lock released here; pointer stays valid (Arc keeps data alive)
            if in_irq {
                jump_to_context_irq(ctx_ptr);
            } else {
                jump_to_context(ctx_ptr);
            }
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

pub fn terminate_current(exit_code: i32) -> ! {
    let mut parent_pid = 0;
    if let Some(arc) = current_process() {
        let mut p = arc.lock();
        parent_pid = p.ppid;
        p.state = ProcessState::Zombie;
        p.exit_code = exit_code;
    }
    if parent_pid != 0 {
        scheduler::wake_up(parent_pid);
    }

    loop {
        schedule();
        let rflags = crate::arch::x86_64::io::read_rflags();
        if rflags & crate::arch::x86_64::io::RFLAGS_IF == 0 {
            crate::arch::x86_64::io::sti();
        }
        crate::arch::x86_64::io::hlt();
    }
}

/// Wake every sleeping process — used by keyboard IRQ so the shell can receive input.
pub fn wake_up_all_sleeping() {
    let rq = RUN_QUEUE.lock();
    for p in &rq.queue {
        let mut proc = p.lock();
        if proc.state == ProcessState::Sleeping {
            proc.state = ProcessState::Runnable;
        }
    }
    // Also wake the current process if it is sleeping (edge case during scheduling).
    if let Some(ref cur) = rq.current {
        let mut proc = cur.lock();
        if proc.state == ProcessState::Sleeping {
            proc.state = ProcessState::Runnable;
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

#[unsafe(naked)]
pub unsafe extern "C" fn context_switch_irq(old: *mut CpuContext, new: *const CpuContext) {
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
        "jmp *56(%rsi)",
        "1:",
        "ret",
        options(att_syntax)
    );
}

#[unsafe(naked)]
unsafe extern "C" fn jump_to_context_irq(ctx: *const CpuContext) {
    core::arch::naked_asm!(
        "mov  0(%rdi),%r15",
        "mov  8(%rdi),%r14",
        "mov 16(%rdi),%r13",
        "mov 24(%rdi),%r12",
        "mov 32(%rdi),%rbp",
        "mov 40(%rdi),%rbx",
        "mov 48(%rdi),%rsp",
        "jmp *56(%rdi)",
        options(att_syntax)
    );
}

/// First entry point for a new user-mode process.
/// The kernel stack was set up with an IRETQ frame by Process::new_user.
/// IRETQ pops: RIP, CS, RFLAGS, RSP, SS → ring 3.
#[unsafe(naked)]
pub unsafe extern "C" fn iretq_trampoline() -> ! {
    core::arch::naked_asm!("iretq", options(att_syntax));
}

pub mod scheduler {
    pub use super::{current_process, schedule, sleep_current, spawn, tick, wake_up, RUN_QUEUE};
}
