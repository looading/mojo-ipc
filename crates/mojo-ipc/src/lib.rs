//! Standalone Cargo facade over the official Chromium Mojo runtime/bindings.
//!
//! Initialize a trusted native SDK library before constructing Mojo objects.
//! The native runtime is process-lifetime; dropping this handle never unloads
//! native code while upstream handles or callbacks may still exist.
pub use bindings;
pub use bindings::receiver::{PendingReceiver, Receiver};
pub use bindings::register_mojom_state_object_impls;
pub use bindings::remote::{PendingRemote, Remote};
pub use raw_ffi::{InitError, RuntimeConfig};
pub use sequenced_task_runner::SequencedTaskRunnerHandle;
use std::{ffi::CString, ptr::NonNull, time::Duration};
pub use system::{self, message_pipe::MessageEndpoint};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Broker,
    Participant,
}

#[derive(Clone, Debug)]
pub struct Runtime {
    _private: (),
}

impl Runtime {
    /// # Safety
    /// The path must be an authentic native SDK image for this source pin.
    /// See `mojo_sys::initialize` for the native-code trust boundary.
    pub unsafe fn load(path: impl AsRef<std::path::Path>, role: Role) -> Result<Self, InitError> {
        // SAFETY: 调用者已满足默认官方原生库的信任前提。
        unsafe { Self::load_with_config(path, role, RuntimeConfig::default()) }
    }

    /// 加载显式选择 Chromium 提交的原生库。
    ///
    /// # Safety
    /// 原生库必须来自可信构建，且 C ABI、生成类型及 thunk 布局与本 SDK 兼容。
    /// 版本字符串校验不构成二进制真实性或兼容性保证。
    pub unsafe fn load_with_config(
        path: impl AsRef<std::path::Path>,
        role: Role,
        config: RuntimeConfig<'_>,
    ) -> Result<Self, InitError> {
        unsafe {
            raw_ffi::initialize_with_config(path.as_ref(), role == Role::Broker, config)?;
        }
        Ok(Self { _private: () })
    }

    pub fn dispatch_runner(&self) -> SequencedTaskRunnerHandle {
        SequencedTaskRunnerHandle::dispatch_runner()
    }

    /// A broker launches a participant and returns its primordial message pipe.
    /// This is a blocking operation. The executable must call accept_inherited.
    /// Program and arguments must be UTF-8 and contain no interior NUL bytes.
    pub fn spawn_worker(
        &self,
        program: &str,
        args: &[&str],
    ) -> Result<(ChildProcess, MessageEndpoint), InitError> {
        let program = cstring(program)?;
        let arguments = args
            .iter()
            .map(|s| cstring(s))
            .collect::<Result<Vec<_>, _>>()?;
        let pointers = arguments.iter().map(|s| s.as_ptr()).collect::<Vec<_>>();
        let options = raw_ffi::MojoRsSpawnOptions {
            struct_size: size_of::<raw_ffi::MojoRsSpawnOptions>() as u32,
            program: program.as_ptr(),
            args: pointers.as_ptr(),
            args_len: pointers.len(),
        };
        let mut pipe = 0;
        let mut child = std::ptr::null_mut();
        let function = raw_ffi::runtime_api()
            .spawn_child
            .ok_or_else(|| InitError("native runtime lacks spawn_child".into()))?;
        status(unsafe { function(&options, &mut pipe, &mut child) })?;
        let child = NonNull::new(child)
            .ok_or_else(|| InitError("native spawn returned no child".into()))?;
        let child = ChildProcess(child);
        let endpoint = endpoint(pipe)?;
        Ok((child, endpoint))
    }

    /// A participant accepts an inherited invitation. Blocks and must not run on the native
    /// IO/dispatch sequence or an async executor worker.
    ///
    /// # Safety
    /// The token must be the unconsumed endpoint actually transferred by the
    /// SDK spawn helper to this process. It grants ownership of native handles;
    /// arbitrary strings or tokens for already-owned handles are not valid.
    pub unsafe fn accept_inherited(&self, token: &str) -> Result<MessageEndpoint, InitError> {
        let token = cstring(token)?;
        let mut pipe = 0;
        let function = raw_ffi::runtime_api()
            .accept_inherited
            .ok_or_else(|| InitError("native runtime lacks accept_inherited".into()))?;
        status(unsafe { function(token.as_ptr(), &mut pipe) })?;
        endpoint(pipe)
    }

    /// Extract the native launch helper's bootstrap argument, ignoring other
    /// arguments that are not Unicode.
    pub fn inherited_token() -> Option<String> {
        std::env::args_os().find_map(|arg| {
            arg.to_str()?
                .strip_prefix("--mojo-rust-sdk-bootstrap=")
                .map(str::to_owned)
        })
    }

    /// A broker creates a single-connection named listener. On macOS this checks in a
    /// service in the process's Mach bootstrap namespace. Namespace policy may
    /// require launchd MachServices deployment; see the native documentation.
    pub fn listen_once(&self, name: &str) -> Result<Listener, InitError> {
        let name = cstring(name)?;
        let mut listener = std::ptr::null_mut();
        let function = raw_ffi::runtime_api()
            .listen_once
            .ok_or_else(|| InitError("native runtime lacks named listeners".into()))?;
        status(unsafe { function(name.as_ptr(), &mut listener) })?;
        NonNull::new(listener)
            .map(Listener)
            .ok_or_else(|| InitError("native listen returned no listener".into()))
    }

    /// A participant connects to a named broker and accepts its invitation. Blocking.
    pub fn connect(&self, name: &str) -> Result<MessageEndpoint, InitError> {
        let name = cstring(name)?;
        let mut pipe = 0;
        let function = raw_ffi::runtime_api()
            .connect_named
            .ok_or_else(|| InitError("native runtime lacks named connections".into()))?;
        status(unsafe { function(name.as_ptr(), &mut pipe) })?;
        endpoint(pipe)
    }
}

pub struct ChildProcess(NonNull<raw_ffi::MojoRsChild>);
impl ChildProcess {
    /// Wait for the child, bounded by timeout. Does not terminate on timeout.
    pub fn wait(&mut self, timeout: Duration) -> Result<i32, InitError> {
        let mut code = 0;
        let function = raw_ffi::runtime_api()
            .child_wait
            .ok_or_else(|| InitError("native runtime lacks child_wait".into()))?;
        status(unsafe { function(self.0.as_ptr(), millis(timeout), &mut code) })?;
        Ok(code)
    }
}
impl Drop for ChildProcess {
    fn drop(&mut self) {
        if let Some(release) = raw_ffi::runtime_api().child_release {
            unsafe { release(self.0.as_ptr()) }
        }
    }
}

pub struct Listener(NonNull<raw_ffi::MojoRsListener>);
impl Listener {
    /// Accept one participant, supplying its PID for handle transfer on
    /// Windows/macOS. On Linux peer_pid may be zero. Blocking and one-shot.
    pub fn accept(
        &mut self,
        peer_pid: u64,
        timeout: Duration,
    ) -> Result<MessageEndpoint, InitError> {
        let mut pipe = 0;
        let function = raw_ffi::runtime_api()
            .listener_accept
            .ok_or_else(|| InitError("native runtime lacks listener_accept".into()))?;
        status(unsafe { function(self.0.as_ptr(), peer_pid, millis(timeout), &mut pipe) })?;
        endpoint(pipe)
    }
}
impl Drop for Listener {
    fn drop(&mut self) {
        if let Some(release) = raw_ffi::runtime_api().listener_release {
            unsafe { release(self.0.as_ptr()) }
        }
    }
}

fn endpoint(raw: usize) -> Result<MessageEndpoint, InitError> {
    if raw == 0 {
        return Err(InitError(
            "native bootstrap returned an invalid pipe".into(),
        ));
    }
    // SAFETY: successful native bootstrap transfers sole ownership of a live
    // message-pipe handle, never a borrowed handle or a platform descriptor.
    Ok(MessageEndpoint::from(unsafe {
        c_mojo_api::UntypedHandle::wrap_raw_value(raw)
    }))
}
fn status(value: u32) -> Result<(), InitError> {
    if value == raw_ffi::MOJO_RESULT_OK {
        Ok(())
    } else {
        Err(InitError(format!("native Mojo operation failed: {value}")))
    }
}
fn cstring(value: &str) -> Result<CString, InitError> {
    CString::new(value).map_err(|_| InitError("native string contains an interior NUL".into()))
}
fn millis(duration: Duration) -> u64 {
    duration.as_millis().min(u64::MAX as u128) as u64
}

#[doc(hidden)]
pub mod __private {
    pub use anyhow;
    pub use bindings;
    pub use mojom_value_parser;
    pub use mojom_value_parser::*;
    pub use mojom_value_parser_core;
    pub use ordered_float;
    pub use system;
}
