//! Pinned official Mojo C types and dispatch through the SDK native API.
//!
//! No Mojo C++ or public-system library is linked into the Rust host. The
//! native image and Core are process-lifetime singletons.
#![allow(non_snake_case, non_camel_case_types, non_upper_case_globals)]

mod generated {
    include!("generated/types.rs");
}
pub use generated::*;
include!("generated/dispatch.rs");

use std::{
    ffi::CStr,
    fmt,
    mem::MaybeUninit,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

pub const CHROMIUM_COMMIT: &str = "bcbc09a9f8e4c3fd3ad2ed9cbad41e63c35ccfdd";
pub const SDK_ABI_VERSION: u32 = 1;

/// 原生库的来源约束；默认使用 SDK 的官方 Chromium 提交。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuntimeConfig<'a> {
    pub chromium_commit: &'a str,
}

impl Default for RuntimeConfig<'_> {
    fn default() -> Self {
        Self {
            chromium_commit: CHROMIUM_COMMIT,
        }
    }
}

impl RuntimeConfig<'_> {
    fn validate(self) -> Result<(), InitError> {
        if self.chromium_commit.len() != 40
            || !self
                .chromium_commit
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(InitError(
                "expected Chromium commit must be 40 lowercase hexadecimal characters".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug)]
pub struct InitError(pub String);
impl fmt::Display for InitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
impl std::error::Error for InitError {}

struct NativeRuntime {
    api: MojoRsApiV1,
    thunks: &'static MojoSystemThunks2,
    path: PathBuf,
    broker: bool,
    chromium_commit: String,
}
// SAFETY: The API table is immutable, its native image is never unloaded, and
// both the official C System API and runner-reference operations are thread-safe.
unsafe impl Send for NativeRuntime {}
unsafe impl Sync for NativeRuntime {}
static RUNTIME: OnceLock<NativeRuntime> = OnceLock::new();
static LOAD_LOCK: Mutex<()> = Mutex::new(());

/// Load a trusted SDK runtime and initialize the process-wide Mojo node.
///
/// # Safety
/// `path` must point to the authentic SDK library built from this exact source
/// revision and ABI. Loading native code and trusting its pointers cannot be
/// made safe by a version string alone. The image is retained until exit.
pub unsafe fn initialize(path: &Path, broker: bool) -> Result<(), InitError> {
    // SAFETY: 调用者承担与原入口相同的原生库来源及 ABI 信任责任。
    unsafe { initialize_with_config(path, broker, RuntimeConfig::default()) }
}

/// 加载与显式 Chromium 提交匹配的原生库，仍检查完整 ABI 和系统函数表。
///
/// # Safety
/// `path` 必须是由调用者信任且审核过的原生库，其 C ABI、Mojo 类型和
/// thunk 布局必须与本 SDK 一致。提交字符串不能替代二进制来源和兼容性审核。
/// 原生库在进程退出前不会卸载。
pub unsafe fn initialize_with_config(
    path: &Path,
    broker: bool,
    config: RuntimeConfig<'_>,
) -> Result<(), InitError> {
    config.validate()?;
    let path = path
        .canonicalize()
        .map_err(|e| InitError(format!("{}: {e}", path.display())))?;
    let _guard = LOAD_LOCK
        .lock()
        .map_err(|_| InitError("native initialization lock poisoned".into()))?;
    if let Some(runtime) = RUNTIME.get() {
        return if runtime.path == path
            && runtime.broker == broker
            && runtime.chromium_commit == config.chromium_commit
        {
            Ok(())
        } else {
            Err(InitError(
                "this process already initialized a different runtime, node role or Chromium revision".into(),
            ))
        };
    }
    let library =
        unsafe { libloading::Library::new(&path) }.map_err(|e| InitError(e.to_string()))?;
    type GetApi = unsafe extern "C" fn(u32, u32, *mut MojoRsApiV1) -> u32;
    let get_api: libloading::Symbol<'_, GetApi> =
        unsafe { library.get(b"MojoRsGetApiV1\0") }.map_err(|e| InitError(e.to_string()))?;
    let mut storage = MaybeUninit::<MojoRsApiV1>::zeroed();
    let status = unsafe {
        get_api(
            SDK_ABI_VERSION,
            size_of::<MojoRsApiV1>() as u32,
            storage.as_mut_ptr(),
        )
    };
    if status != MOJO_RESULT_OK {
        return Err(InitError(format!("GetApiV1 failed: {status}")));
    }
    let api = unsafe { storage.assume_init() };
    if api.struct_size as usize != size_of::<MojoRsApiV1>() || api.abi_version != SDK_ABI_VERSION {
        return Err(InitError("native API size/version mismatch".into()));
    }
    if api.chromium_commit.is_null()
        || unsafe { CStr::from_ptr(api.chromium_commit) }.to_bytes()
            != config.chromium_commit.as_bytes()
    {
        return Err(InitError("native Chromium revision mismatch".into()));
    }
    let init = api
        .runtime_init
        .ok_or_else(|| InitError("missing runtime_init".into()))?;
    let get_thunks = api
        .system_thunks
        .ok_or_else(|| InitError("missing system_thunks".into()))?;
    if api.dispatch_runner.is_none()
        || api.current_runner.is_none()
        || api.runner_retain.is_none()
        || api.runner_release.is_none()
        || api.runner_is_current.is_none()
        || api.runner_post.is_none()
        || api.time_ticks_now_micros.is_none()
        || api.next_trace_id.is_none()
        || api.spawn_child.is_none()
        || api.accept_inherited.is_none()
        || api.child_wait.is_none()
        || api.child_release.is_none()
        || api.listen_once.is_none()
        || api.listener_accept.is_none()
        || api.listener_release.is_none()
        || api.connect_named.is_none()
    {
        return Err(InitError("incomplete native sequence/helper API".into()));
    }
    // A failed initialization can have started native threads. Retain the
    // image before calling init, including on the error path.
    let _retained = Box::leak(Box::new(library));
    let options = MojoRsInitOptions {
        struct_size: size_of::<MojoRsInitOptions>() as u32,
        is_broker: broker as u32,
    };
    let status = unsafe { init(&options) };
    if status != MOJO_RESULT_OK {
        return Err(InitError(format!("runtime_init failed: {status}")));
    }
    let pointer = unsafe { get_thunks() }.cast::<MojoSystemThunks2>();
    if pointer.is_null() {
        return Err(InitError("native runtime returned no system table".into()));
    }
    // Inspect the size prefix before creating a reference to the full table.
    let size = unsafe { std::ptr::read(pointer.cast::<u32>()) };
    if (size as usize) < size_of::<MojoSystemThunks2>() {
        return Err(InitError("incomplete Mojo System thunk table".into()));
    }
    let thunks = unsafe { &*pointer };
    RUNTIME
        .set(NativeRuntime {
            api,
            thunks,
            path,
            broker,
            chromium_commit: config.chromium_commit.to_owned(),
        })
        .map_err(|_| InitError("runtime initialized concurrently".into()))?;
    Ok(())
}

pub fn is_initialized() -> bool {
    RUNTIME.get().is_some()
}

#[doc(hidden)]
pub fn runtime_api() -> &'static MojoRsApiV1 {
    &RUNTIME
        .get()
        .expect("initialize a Mojo Runtime before using this API")
        .api
}

fn system_thunks() -> &'static MojoSystemThunks2 {
    RUNTIME
        .get()
        .expect("initialize a Mojo Runtime before using handles")
        .thunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_runtime_config_keeps_the_official_chromium_pin() {
        assert_eq!(RuntimeConfig::default().chromium_commit, CHROMIUM_COMMIT);
        assert!(RuntimeConfig::default().validate().is_ok());
    }

    #[test]
    fn custom_runtime_config_requires_an_exact_commit() {
        let custom = RuntimeConfig {
            chromium_commit: "6d2bf4f33d77670349c45ffa1464bef101c0eda5",
        };
        assert!(custom.validate().is_ok());
        for invalid in [
            "",
            "main",
            "6d2bf4f",
            " bcbc09a9f8e4c3fd3ad2ed9cbad41e63c35ccfdd",
            "BCBC09A9F8E4C3FD3AD2ED9CBAD41E63C35CCFDD",
            "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz",
        ] {
            assert!(
                RuntimeConfig {
                    chromium_commit: invalid
                }
                .validate()
                .is_err(),
                "{invalid}"
            );
        }
    }
}
