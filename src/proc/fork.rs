use crate::arch::x86_64::io::invlpg;
use crate::arch::x86_64::limine::{phys_to_virt, virt_to_phys};
use crate::mm::pmm::{align_up, alloc_zeroed_frame, free_frame, PAGE_SIZE};
use crate::mm::vmm::{
    AddressSpace, PageTable, VmSpace, VmaEntry, VmaFlags, PTE_ADDR_MASK, PTE_NO_EXEC, PTE_PRESENT,
    PTE_USER, PTE_WRITABLE,
};
use crate::proc::{alloc_pid, Process, ProcessState};
use crate::sync::spinlock::SpinLock;
use alloc::sync::Arc;
use alloc::vec::Vec;

pub fn clone_address_space(
    parent_space: &AddressSpace,
    parent_vm: &VmSpace,
) -> Option<(AddressSpace, VmSpace)> {
    let child_pml4_phys = alloc_zeroed_frame()?;

    let parent_pml4 = unsafe { &*(phys_to_virt(parent_space.pml4_phys) as *const PageTable) };
    let child_pml4 = unsafe { &mut *(phys_to_virt(child_pml4_phys) as *mut PageTable) };

    for i in 0..256usize {
        if !parent_pml4.is_present(i) {
            continue;
        }

        let pdpt_phys = clone_pdpt(parent_pml4.entries[i])?;
        child_pml4.entries[i] = pdpt_phys;
    }

    for i in 256..512usize {
        child_pml4.entries[i] = parent_pml4.entries[i];
    }

    make_cow_pml4(parent_space.pml4_phys);

    unsafe {
        crate::arch::x86_64::io::flush_tlb_all();
    }

    let child_vm = clone_vmspace(parent_vm);

    let child_space = AddressSpace {
        pml4_phys: child_pml4_phys,
    };

    Some((child_space, child_vm))
}

fn clone_pdpt(parent_entry: u64) -> Option<u64> {
    let parent_phys = parent_entry & PTE_ADDR_MASK;
    let flags = parent_entry & !PTE_ADDR_MASK;

    let parent_pdpt = unsafe { &*(phys_to_virt(parent_phys) as *const PageTable) };

    let child_phys = alloc_zeroed_frame()?;
    let child_pdpt = unsafe { &mut *(phys_to_virt(child_phys) as *mut PageTable) };

    for i in 0..512usize {
        if !parent_pdpt.is_present(i) {
            continue;
        }
        let pd_phys = clone_pd(parent_pdpt.entries[i])?;
        child_pdpt.entries[i] = pd_phys;
    }

    Some(child_phys | flags)
}

fn clone_pd(parent_entry: u64) -> Option<u64> {
    if parent_entry & crate::mm::vmm::PTE_LARGE != 0 {
        return Some(parent_entry & !PTE_WRITABLE);
    }

    let parent_phys = parent_entry & PTE_ADDR_MASK;
    let flags = parent_entry & !PTE_ADDR_MASK;

    let parent_pd = unsafe { &*(phys_to_virt(parent_phys) as *const PageTable) };

    let child_phys = alloc_zeroed_frame()?;
    let child_pd = unsafe { &mut *(phys_to_virt(child_phys) as *mut PageTable) };

    for i in 0..512usize {
        if !parent_pd.is_present(i) {
            continue;
        }
        let pt_phys = clone_pt(parent_pd.entries[i])?;
        child_pd.entries[i] = pt_phys;
    }

    Some(child_phys | flags)
}

fn clone_pt(parent_entry: u64) -> Option<u64> {
    let parent_phys = parent_entry & PTE_ADDR_MASK;
    let flags = parent_entry & !PTE_ADDR_MASK;

    let parent_pt = unsafe { &*(phys_to_virt(parent_phys) as *const PageTable) };

    let child_phys = alloc_zeroed_frame()?;
    let child_pt = unsafe { &mut *(phys_to_virt(child_phys) as *mut PageTable) };

    for i in 0..512usize {
        if !parent_pt.is_present(i) {
            continue;
        }

        let pte = parent_pt.entries[i];
        let cow_pte = if pte & PTE_USER != 0 {
            pte & !PTE_WRITABLE
        } else {
            pte
        };

        child_pt.entries[i] = cow_pte;
    }

    Some(child_phys | flags)
}

fn make_cow_pml4(pml4_phys: u64) {
    let pml4 = unsafe { &mut *(phys_to_virt(pml4_phys) as *mut PageTable) };

    for i in 0..256usize {
        if !pml4.is_present(i) {
            continue;
        }
        let pdpt_phys = pml4.entries[i] & PTE_ADDR_MASK;
        make_cow_pdpt(pdpt_phys);
    }
}

fn make_cow_pdpt(pdpt_phys: u64) {
    let pdpt = unsafe { &mut *(phys_to_virt(pdpt_phys) as *mut PageTable) };
    for i in 0..512usize {
        if !pdpt.is_present(i) {
            continue;
        }
        let pd_phys = pdpt.entries[i] & PTE_ADDR_MASK;
        make_cow_pd(pd_phys);
    }
}

fn make_cow_pd(pd_phys: u64) {
    let pd = unsafe { &mut *(phys_to_virt(pd_phys) as *mut PageTable) };
    for i in 0..512usize {
        if !pd.is_present(i) {
            continue;
        }
        if pd.entries[i] & crate::mm::vmm::PTE_LARGE != 0 {
            pd.entries[i] &= !PTE_WRITABLE;
            continue;
        }
        let pt_phys = pd.entries[i] & PTE_ADDR_MASK;
        make_cow_pt(pt_phys);
    }
}

fn make_cow_pt(pt_phys: u64) {
    let pt = unsafe { &mut *(phys_to_virt(pt_phys) as *mut PageTable) };
    for i in 0..512usize {
        if !pt.is_present(i) {
            continue;
        }
        if pt.entries[i] & PTE_USER != 0 {
            pt.entries[i] &= !PTE_WRITABLE;
        }
    }
}

fn clone_vmspace(parent: &VmSpace) -> VmSpace {
    let mut child = VmSpace::new();
    child.brk = parent.brk;

    for vma in &parent.areas {
        let mut flags = vma.flags;
        if flags.contains(VmaFlags::WRITE) && flags.contains(VmaFlags::ANONYMOUS) {
            flags |= VmaFlags::COPY_ON_WRITE;
        }
        child.areas.push(VmaEntry {
            start: vma.start,
            end: vma.end,
            flags,
        });
    }

    child
}

pub fn sys_fork(parent_frame: &crate::arch::x86_64::idt::InterruptFrame) -> i64 {
    use crate::proc::scheduler;

    let parent_arc = match scheduler::current_process() {
        Some(p) => p,
        None => return -crate::syscall::errno::EINVAL,
    };

    let child_pid = alloc_pid();

    let (child_space, child_vm, child_context, child_stack, base_slice, priority, name) = {
        let parent = parent_arc.lock();

        let (space, vm) = match clone_address_space(&parent.address_space, &parent.vm) {
            Some(r) => r,
            None => return -crate::syscall::errno::ENOMEM,
        };

        let mut ctx = parent.context.clone();

        use crate::arch::x86_64::limine::phys_to_virt;
        use crate::mm::pmm::alloc_frames;
        let kstack_phys = match alloc_frames(Process::KERNEL_STACK_SIZE / PAGE_SIZE) {
            Some(p) => p,
            None => return -crate::syscall::errno::ENOMEM,
        };
        let kstack_virt = phys_to_virt(kstack_phys);
        let kstack_top = kstack_virt + Process::KERNEL_STACK_SIZE as u64;

        ctx.rsp = kstack_top;
        ctx.rip = fork_child_return as u64;

        (
            space,
            vm,
            ctx,
            kstack_virt,
            parent.base_slice,
            parent.priority,
            parent.name,
        )
    };

    let child = Process {
        pid: child_pid,
        ppid: parent_arc.lock().pid,
        state: ProcessState::Runnable,
        context: child_context,
        address_space: child_space,
        vm: child_vm,
        kernel_stack: child_stack,
        kernel_stack_size: Process::KERNEL_STACK_SIZE,
        priority,
        time_slice: base_slice,
        base_slice,
        exit_code: 0,
        name,
        pending_signals: 0,
        signal_mask: 0,
    };

    let child_arc = Arc::new(SpinLock::new(child));

    scheduler::spawn(child_arc);

    log::info!(
        "fork(): parent={} child={}",
        parent_arc.lock().pid,
        child_pid
    );

    child_pid as i64
}

#[unsafe(naked)]
unsafe extern "C" fn fork_child_return() {
    core::arch::naked_asm!(
        "xor %rax, %rax",
        "pop %r15",
        "pop %r14",
        "pop %r13",
        "pop %r12",
        "pop %rbp",
        "pop %rbx",
        "pop %rcx",
        "pop %r11",
        "mov %gs:16, %rsp",
        "cli",
        "swapgs",
        "sysretq",
        options(att_syntax)
    );
}

pub fn sys_waitpid(pid: i32, wstatus_ptr: u64, options: u32) -> i64 {
    const WNOHANG: u32 = 1;

    loop {
        let found = find_zombie_child(pid);

        if let Some((child_pid, exit_code)) = found {
            if wstatus_ptr != 0 {
                let wstatus = ((exit_code & 0xFF) as u32) << 8;
                let phys = crate::proc::scheduler::current_process()
                    .and_then(|p| p.lock().address_space.translate(wstatus_ptr));
                if let Some(phys) = phys {
                    unsafe {
                        *(phys_to_virt(phys) as *mut u32) = wstatus;
                    }
                }
            }

            reap_zombie(child_pid);

            return child_pid as i64;
        }

        if options & WNOHANG != 0 {
            return 0;
        }

        crate::proc::scheduler::sleep_current();
    }
}

fn find_zombie_child(target_pid: i32) -> Option<(u32, i32)> {
    use crate::proc::scheduler::RUN_QUEUE;
    let rq = RUN_QUEUE.lock();
    let current_pid = rq.current.as_ref()?.lock().pid;

    for proc_arc in &rq.queue {
        let proc = proc_arc.lock();
        if proc.ppid != current_pid {
            continue;
        }
        if target_pid != -1 && proc.pid != target_pid as u32 {
            continue;
        }
        if proc.state == ProcessState::Zombie {
            return Some((proc.pid, proc.exit_code));
        }
    }
    None
}

fn reap_zombie(pid: u32) {
    use crate::proc::scheduler::RUN_QUEUE;
    let mut rq = RUN_QUEUE.lock();
    rq.queue.retain(|p| p.lock().pid != pid);
}

pub fn sys_fork_simple() -> i64 {
    use crate::proc::scheduler;
    use crate::sync::spinlock::SpinLock;

    let parent_arc = match scheduler::current_process() {
        Some(p) => p,
        None => return -crate::syscall::errno::EINVAL,
    };

    let child_pid = alloc_pid();

    let result = {
        let parent = parent_arc.lock();

        let (space, vm) = match clone_address_space(&parent.address_space, &parent.vm) {
            Some(r) => r,
            None => return -crate::syscall::errno::ENOMEM,
        };

        use crate::arch::x86_64::limine::phys_to_virt;
        use crate::mm::pmm::alloc_frames;
        let kstack_phys = match alloc_frames(Process::KERNEL_STACK_SIZE / PAGE_SIZE) {
            Some(p) => p,
            None => return -crate::syscall::errno::ENOMEM,
        };
        let kstack_virt = phys_to_virt(kstack_phys);
        let kstack_top = kstack_virt + Process::KERNEL_STACK_SIZE as u64;

        let mut ctx = parent.context.clone();
        ctx.rsp = kstack_top;
        ctx.rip = fork_child_return as u64;

        let child = Process {
            pid: child_pid,
            ppid: parent.pid,
            state: ProcessState::Runnable,
            context: ctx,
            address_space: space,
            vm,
            kernel_stack: kstack_virt,
            kernel_stack_size: Process::KERNEL_STACK_SIZE,
            priority: parent.priority,
            time_slice: parent.base_slice,
            base_slice: parent.base_slice,
            exit_code: 0,
            name: parent.name,
            pending_signals: 0,
            signal_mask: parent.signal_mask,
        };

        Arc::new(SpinLock::new(child))
    };

    scheduler::spawn(result);
    log::info!("fork() -> child pid={}", child_pid);
    child_pid as i64
}
