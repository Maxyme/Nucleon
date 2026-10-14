//! Publish original trampolines before activating detours, with one installation per group.
use anyhow::{Result, ensure};
use frida_gum::{NativePointer, interceptor::Interceptor};
use std::{
    ffi::c_void,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicPtr, Ordering},
    },
};

// All Nucleon users share Gum's interceptor. Do not overlap transactions from separate groups.
static INSTALL_LOCK: Mutex<()> = Mutex::new(());

pub(super) struct Original(AtomicPtr<c_void>);
impl Original {
    pub const fn new() -> Self {
        Self(AtomicPtr::new(std::ptr::null_mut()))
    }
    pub fn load(&self) -> Option<*mut c_void> {
        let pointer = self.0.load(Ordering::Acquire);
        (!pointer.is_null()).then_some(pointer)
    }
    fn publish(&self, pointer: NativePointer) {
        self.0.store(pointer.0, Ordering::Release);
    }
}

/// Cache failures too: retrying a partially prepared group could replace live trampolines.
pub(super) fn once(
    state: &OnceLock<std::result::Result<(), String>>,
    install: impl FnOnce() -> Result<()>,
) -> Result<()> {
    state
        .get_or_init(|| install().map_err(|e| format!("{e:#}")))
        .as_ref()
        .map(|_| ())
        .map_err(|e| anyhow::anyhow!("{e}"))
}

pub(super) struct Transaction<'a> {
    interceptor: Interceptor,
    replacements: Vec<NativePointer>,
    committed: bool,
    _lock: std::sync::MutexGuard<'a, ()>,
}
impl Transaction<'static> {
    pub fn begin() -> Result<Self> {
        let lock = INSTALL_LOCK
            .lock()
            .map_err(|_| anyhow::anyhow!("Hook installation lock poisoned"))?;
        let mut interceptor = Interceptor::obtain(crate::get_gum());
        interceptor.begin_transaction();
        Ok(Self {
            interceptor,
            replacements: Vec::new(),
            committed: false,
            _lock: lock,
        })
    }
}
impl Transaction<'_> {
    /// Addresses must identify functions with the same native ABI; checked by each caller.
    pub fn replace(
        &mut self,
        target: NativePointer,
        replacement: NativePointer,
    ) -> Result<NativePointer> {
        let pointer = self
            .interceptor
            .replace_fast(target, replacement)
            .map_err(|e| anyhow::anyhow!("Frida detour failed: {e:?}"))?;
        self.replacements.push(target);
        ensure!(!pointer.0.is_null(), "Frida returned a null trampoline");
        Ok(pointer)
    }
    pub fn commit(&mut self) {
        self.committed = true;
    }
}
impl Drop for Transaction<'_> {
    fn drop(&mut self) {
        if !self.committed {
            for target in self.replacements.iter().rev() {
                self.interceptor.revert(*target);
            }
        }
        self.interceptor.end_transaction();
    }
}

pub(super) fn install(bindings: &[(NativePointer, NativePointer, &Original)]) -> Result<()> {
    let mut transaction = Transaction::begin()?;
    let mut prepared = Vec::with_capacity(bindings.len());
    for (target, replacement, original) in bindings {
        ensure!(
            original.load().is_none(),
            "Original trampoline already published"
        );
        prepared.push(transaction.replace(*target, *replacement)?);
    }
    for ((_, _, original), pointer) in bindings.iter().zip(prepared) {
        original.publish(pointer);
    }
    // end_transaction (Drop) activates detours only after every Release store above.
    transaction.commit();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, atomic::AtomicUsize};

    #[test]
    fn concurrent_installation_runs_once_and_caches_failure() {
        for fail in [false, true] {
            let state = Arc::new(OnceLock::new());
            let count = Arc::new(AtomicUsize::new(0));
            std::thread::scope(|scope| {
                for _ in 0..16 {
                    let state = &state;
                    let count = &count;
                    scope.spawn(move || {
                        let result = once(state, || {
                            count.fetch_add(1, Ordering::Relaxed);
                            std::thread::yield_now();
                            if fail {
                                anyhow::bail!("installation failed");
                            }
                            Ok(())
                        });
                        assert_eq!(result.is_err(), fail);
                    });
                }
            });
            assert_eq!(count.load(Ordering::Relaxed), 1);
        }
    }

    static ORIGINAL: Original = Original::new();
    static MISSING: AtomicUsize = AtomicUsize::new(0);
    #[inline(never)]
    extern "C" fn target(value: usize) -> usize {
        std::hint::black_box(value).wrapping_add(1)
    }
    #[inline(never)]
    extern "C" fn replacement(value: usize) -> usize {
        let Some(pointer) = ORIGINAL.load() else {
            MISSING.fetch_add(1, Ordering::Relaxed);
            return usize::MAX;
        };
        // SAFETY: trampoline from target has this exact ABI and remains installed for the test.
        let original: extern "C" fn(usize) -> usize = unsafe { std::mem::transmute(pointer) };
        original(value).wrapping_add(10)
    }

    #[test]
    fn live_calls_during_transaction_never_see_an_unpublished_original() {
        let mut transaction = Transaction::begin().unwrap();
        let target_ptr = NativePointer(target as *mut c_void);
        let pointer = transaction
            .replace(target_ptr, NativePointer(replacement as *mut c_void))
            .unwrap();
        // Native calls during preparation must still execute the original entry point.
        assert_eq!(target(std::hint::black_box(1)), 2);
        let stop = std::sync::atomic::AtomicBool::new(false);
        let before = AtomicUsize::new(0);
        let after = AtomicUsize::new(0);
        let wait_for_calls = |count: &AtomicUsize| {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while count.load(Ordering::Acquire) < 100 {
                if std::time::Instant::now() >= deadline {
                    stop.store(true, Ordering::Release);
                    panic!("Native caller did not observe the expected phase");
                }
                std::thread::yield_now();
            }
        };
        std::thread::scope(|scope| {
            let worker = scope.spawn(|| {
                while !stop.load(Ordering::Acquire) {
                    let value = target(std::hint::black_box(1));
                    match value {
                        2 => {
                            before.fetch_add(1, Ordering::Release);
                        }
                        12 => {
                            after.fetch_add(1, Ordering::Release);
                        }
                        _ => {
                            stop.store(true, Ordering::Release);
                            panic!("invalid trampoline result {value}");
                        }
                    }
                }
            });
            wait_for_calls(&before);
            ORIGINAL.publish(pointer);
            transaction.commit();
            drop(transaction);
            assert_eq!(target(std::hint::black_box(1)), 12);
            wait_for_calls(&after);
            stop.store(true, Ordering::Release);
            worker.join().unwrap();
        });
        assert_eq!(MISSING.load(Ordering::Relaxed), 0);
        // Keep the callable trampoline alive for the process lifetime, like production hooks.
    }

    #[inline(never)]
    extern "C" fn rollback_target(value: usize) -> usize {
        std::hint::black_box(value).wrapping_add(3)
    }
    #[test]
    fn failed_group_rolls_back_prepared_detours_without_publishing() {
        let first = Original::new();
        let second = Original::new();
        let target = NativePointer(rollback_target as *mut c_void);
        let replacement = NativePointer(replacement as *mut c_void);
        // Gum rejects the second replacement of the same entry, after preparing the first.
        assert!(
            install(&[
                (target, replacement, &first),
                (target, replacement, &second)
            ])
            .is_err()
        );
        assert!(first.load().is_none());
        assert!(second.load().is_none());
        assert_eq!(rollback_target(std::hint::black_box(1)), 4);
        // The failed group ended its transaction, so another replacement can be prepared.
        let mut retry = Transaction::begin().unwrap();
        retry.replace(target, replacement).unwrap();
        drop(retry);
        assert_eq!(rollback_target(std::hint::black_box(1)), 4);
    }
}
