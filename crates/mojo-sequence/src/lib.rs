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
        let task: Box<Box<dyn FnOnce() + Send>> = Box::new(Box::new(task));
        let context = Box::into_raw(task).cast::<c_void>();
        let result = unsafe {
            raw_ffi::runtime_api().runner_post.unwrap()(
                self.0.as_ptr(),
                context,
                Some(run_task),
                Some(cancel_task),
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

unsafe extern "C" fn run_task(context: *mut c_void) {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // SAFETY: runner_post consumes the thin Box pointer, and invokes
        // exactly one of run/cancel once with this original pointer.
        let task = unsafe { Box::from_raw(context.cast::<Box<dyn FnOnce() + Send>>()) };
        task();
    }));
    if result.is_err() {
        std::process::abort();
    }
}
unsafe extern "C" fn cancel_task(context: *mut c_void) {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        drop(unsafe { Box::from_raw(context.cast::<Box<dyn FnOnce() + Send>>()) });
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
    #[test]
    fn ordinary_host_thread_has_no_default_sequence() {
        assert!(super::SequencedTaskRunnerHandle::get_current_default().is_none());
    }
}
