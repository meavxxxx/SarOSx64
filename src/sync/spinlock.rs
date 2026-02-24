use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, Ordering};

pub struct SpinLock<T> {
    locked: AtomicBool,
    data: UnsafeCell<T>,
}

unsafe impl<T: Send> Send for SpinLock<T> {}
unsafe impl<T: Send> Sync for SpinLock<T> {}

impl<T> SpinLock<T> {
    pub const fn new(val: T) -> Self {
        Self {
            locked: AtomicBool::new(false),
            data: UnsafeCell::new(val),
        }
    }

    pub fn lock(&self) -> SpinGuard<'_, T> {
        let rflags = crate::arch::x86_64::io::cli();

        loop {
            if self
                .locked
                .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                break;
            }

            core::hint::spin_loop();
        }

        SpinGuard { lock: self, rflags }
    }

    pub fn try_lock(&self) -> Option<SpinGuard<'_, T>> {
        let rflags = crate::arch::x86_64::io::cli();
        if self
            .locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            Some(SpinGuard { lock: self, rflags })
        } else {
            if rflags & crate::arch::x86_64::io::RFLAGS_IF != 0 {
                crate::arch::x86_64::io::sti();
            }
            None
        }
    }

    pub unsafe fn get_mut_unchecked(&self) -> &mut T {
        &mut *self.data.get()
    }
}

pub struct SpinGuard<'a, T> {
    lock: &'a SpinLock<T>,
    rflags: u64,
}

impl<'a, T> Drop for SpinGuard<'a, T> {
    fn drop(&mut self) {
        self.lock.locked.store(false, Ordering::Release);
        if self.rflags & crate::arch::x86_64::io::RFLAGS_IF != 0 {
            crate::arch::x86_64::io::sti();
        }
    }
}

impl<'a, T> Deref for SpinGuard<'a, T> {
    type Target = T;
    fn deref(&self) -> &T {
        unsafe { &*self.lock.data.get() }
    }
}

impl<'a, T> DerefMut for SpinGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.lock.data.get() }
    }
}

use core::sync::atomic::AtomicI32;

pub struct RwSpinLock<T> {
    state: AtomicI32,
    data: UnsafeCell<T>,
}

unsafe impl<T: Send> Send for RwSpinLock<T> {}
unsafe impl<T: Send + Sync> Sync for RwSpinLock<T> {}

impl<T> RwSpinLock<T> {
    pub const fn new(val: T) -> Self {
        Self {
            state: AtomicI32::new(0),
            data: UnsafeCell::new(val),
        }
    }

    pub fn read(&self) -> ReadGuard<'_, T> {
        loop {
            let s = self.state.load(Ordering::Relaxed);
            if s >= 0 {
                if self
                    .state
                    .compare_exchange_weak(s, s + 1, Ordering::Acquire, Ordering::Relaxed)
                    .is_ok()
                {
                    return ReadGuard { lock: self };
                }
            }
            core::hint::spin_loop();
        }
    }

    pub fn write(&self) -> WriteGuard<'_, T> {
        loop {
            if self
                .state
                .compare_exchange_weak(0, -1, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                return WriteGuard { lock: self };
            }
            core::hint::spin_loop();
        }
    }
}

pub struct ReadGuard<'a, T> {
    lock: &'a RwSpinLock<T>,
}
pub struct WriteGuard<'a, T> {
    lock: &'a RwSpinLock<T>,
}

impl<'a, T> Drop for ReadGuard<'a, T> {
    fn drop(&mut self) {
        self.lock.state.fetch_sub(1, Ordering::Release);
    }
}
impl<'a, T> Drop for WriteGuard<'a, T> {
    fn drop(&mut self) {
        self.lock.state.store(0, Ordering::Release);
    }
}
impl<'a, T> Deref for ReadGuard<'a, T> {
    type Target = T;
    fn deref(&self) -> &T {
        unsafe { &*self.lock.data.get() }
    }
}
impl<'a, T> Deref for WriteGuard<'a, T> {
    type Target = T;
    fn deref(&self) -> &T {
        unsafe { &*self.lock.data.get() }
    }
}
impl<'a, T> DerefMut for WriteGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.lock.data.get() }
    }
}
