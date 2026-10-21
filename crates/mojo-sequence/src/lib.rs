//! Rust ownership adapter for the native base::SequencedTaskRunner.
use std::{ffi::c_void, ptr::NonNull};

pub struct SequencedTaskRunnerHandle(NonNull<raw_ffi::MojoRsTaskRunner>);
// SAFETY: native task runners use thread-safe reference counts and PostTask.
unsafe impl Send for SequencedTaskRunnerHandle {}
unsafe impl Sync for SequencedTaskRunnerHandle {}

impl SequencedTaskRunnerHandle {
    /// Returns the current base sequence, if this is a native dispatch thread.
    pub fn get_current_default() -> Option<Self> {
        if !raw_ffi::is_initialized() {
            return None;
        }
        NonNull::new(unsafe { raw_ffi::runtime_api().current_runner.unwrap()() }).map(Self)
    }

    /// Returns a retained reference to the runtime-owned dispatch sequence.
    pub fn dispatch_runner() -> Self {
        Self(
            NonNull::new(unsafe { raw_ffi::runtime_api().dispatch_runner.unwrap()() })
                .expect("initialized native runtime must provide a dispatch sequence"),
        )
    }

    pub fn runs_tasks_in_current_sequence(&self) -> bool {
        unsafe { raw_ffi::runtime_api().runner_is_current.unwrap()(self.0.as_ptr()) != 0 }
    }

    /// Transfers the task to native ownership, including on rejection.
    /// Panics in a task or its destructor abort, rather than cross the C ABI.
    pub fn post_task<F: FnOnce() + Send + 'static>(&self, task: F) -> bool {
        let context = task_context(task);
        let result = unsafe {
            raw_ffi::runtime_api().runner_post.unwrap()(
                self.0.as_ptr(),
                context,
                Some(run_task::<F>),
                Some(cancel_task::<F>),
            )
        };
        result == raw_ffi::MOJO_RESULT_OK
    }
}

impl Clone for SequencedTaskRunnerHandle {
    fn clone(&self) -> Self {
        unsafe { raw_ffi::runtime_api().runner_retain.unwrap()(self.0.as_ptr()) };
        Self(self.0)
    }
}
impl Drop for SequencedTaskRunnerHandle {
    fn drop(&mut self) {
        unsafe { raw_ffi::runtime_api().runner_release.unwrap()(self.0.as_ptr()) };
    }
}
impl std::fmt::Debug for SequencedTaskRunnerHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("SequencedTaskRunnerHandle")
            .field(&self.0)
            .finish()
    }
}

fn task_context<F: FnOnce() + Send + 'static>(task: F) -> *mut c_void {
    // F is sized, so Box<F> has a thin pointer and needs only one allocation.
    // The matching monomorphized callbacks retain its exact concrete type.
    Box::into_raw(Box::new(task)).cast::<c_void>()
}

unsafe extern "C" fn run_task<F: FnOnce() + Send + 'static>(context: *mut c_void) {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // SAFETY: runner_post consumes the thin Box pointer, and invokes
        // exactly one of run/cancel once with this original pointer.
        let task = unsafe { Box::from_raw(context.cast::<F>()) };
        task();
    }));
    if result.is_err() {
        std::process::abort();
    }
}
unsafe extern "C" fn cancel_task<F: FnOnce() + Send + 'static>(context: *mut c_void) {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // SAFETY: cancellation receives the original Box<F> pointer once,
        // instead of (and never after) run_task::<F>.
        drop(unsafe { Box::from_raw(context.cast::<F>()) });
    }));
    if result.is_err() {
        std::process::abort();
    }
}

/// Compatibility surface for the upstream message-header helpers.
#[allow(non_snake_case)]
pub mod ffi {
    pub fn GetNextGlobalTraceId() -> u64 {
        unsafe { raw_ffi::runtime_api().next_trace_id.unwrap()() }
    }
    pub fn CurrentTimeTicksInMicroseconds() -> i64 {
        unsafe { raw_ffi::runtime_api().time_ticks_now_micros.unwrap()() }
    }
}

#[cfg(test)]
mod tests {
    use super::{cancel_task, run_task, task_context};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    struct DropSignal(Arc<AtomicUsize>);
    impl Drop for DropSignal {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn finish_directly<F: FnOnce() + Send + 'static>(task: F, run: bool) {
        let context = task_context(task);
        // SAFETY: this test acts as the native owner, invoking exactly one
        // matching callback once with its original concrete Box<F> pointer.
        unsafe {
            if run {
                run_task::<F>(context);
            } else {
                cancel_task::<F>(context);
            }
        }
    }

    #[test]
    fn concrete_task_runs_once_and_releases_owned_captures() {
        let calls = Arc::new(AtomicUsize::new(0));
        let drops = Arc::new(AtomicUsize::new(0));
        let called = calls.clone();
        let capture = DropSignal(drops.clone());
        let owned = String::from("owned FnOnce capture");
        finish_directly(
            move || {
                assert_eq!(owned, "owned FnOnce capture");
                called.fetch_add(1, Ordering::SeqCst);
                drop((owned, capture));
            },
            true,
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn concrete_task_cancellation_drops_without_running() {
        let calls = Arc::new(AtomicUsize::new(0));
        let drops = Arc::new(AtomicUsize::new(0));
        let called = calls.clone();
        let capture = DropSignal(drops.clone());
        let owned = vec![42u8; 257];
        finish_directly(
            move || {
                called.fetch_add(1, Ordering::SeqCst);
                drop((owned, capture));
            },
            false,
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn zero_sized_task_context_supports_run_and_cancel() {
        static CALLS: AtomicUsize = AtomicUsize::new(0);
        CALLS.store(0, Ordering::SeqCst);
        let task = || {
            CALLS.fetch_add(1, Ordering::SeqCst);
        };
        assert_eq!(std::mem::size_of_val(&task), 0);
        finish_directly(task, true);
        assert_eq!(CALLS.load(Ordering::SeqCst), 1);
        finish_directly(task, false);
        assert_eq!(CALLS.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn ordinary_host_thread_has_no_default_sequence() {
        assert!(super::SequencedTaskRunnerHandle::get_current_default().is_none());
    }
}
