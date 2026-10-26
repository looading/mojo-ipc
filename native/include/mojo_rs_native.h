#ifndef MOJO_RUST_SDK_NATIVE_INCLUDE_MOJO_RS_NATIVE_H_
#define MOJO_RUST_SDK_NATIVE_INCLUDE_MOJO_RS_NATIVE_H_

#include <stddef.h>
#include <stdint.h>

#if defined(_WIN32)
#if defined(MOJO_RS_NATIVE_IMPLEMENTATION)
#if defined(_M_IX86) || defined(__i386__)
/* The x86 .def exports one undecorated ABI1 entry instead of its C name. */
#define MOJO_RS_EXPORT
#else
#define MOJO_RS_EXPORT __declspec(dllexport)
#endif
#else
#define MOJO_RS_EXPORT __declspec(dllimport)
#endif
#else
#define MOJO_RS_EXPORT __attribute__((visibility("default")))
#endif

#ifdef __cplusplus
extern "C" {
#endif

/* Status values are MojoResult values. No C++/Rust unwind may cross this ABI. */
typedef uint32_t MojoRsStatus;
typedef uintptr_t MojoRsHandle;
typedef struct MojoRsTaskRunner MojoRsTaskRunner;
typedef struct MojoRsChild MojoRsChild;
typedef struct MojoRsListener MojoRsListener;
typedef void (*MojoRsTaskCallback)(void* context);

typedef struct MojoRsInitOptions {
  uint32_t struct_size;
  uint32_t is_broker;
} MojoRsInitOptions;

typedef struct MojoRsSpawnOptions {
  uint32_t struct_size;
  /* All strings are NUL-terminated UTF-8. args excludes argv[0]. */
  const char* program;
  const char* const* args;
  size_t args_len;
} MojoRsSpawnOptions;

typedef struct MojoRsApiV1 {
  uint32_t struct_size;
  uint32_t abi_version;
  /* Static, NUL-terminated; owned by the loaded native library. */
  const char* chromium_commit;
  /* Initialization is process-wide. Repeating the same role is idempotent.
     Changing role fails. The library and Core remain loaded until process exit. */
  MojoRsStatus (*runtime_init)(const MojoRsInitOptions* options);
  /* Returns const MojoSystemThunks2*. NULL until runtime_init succeeds.
     The exact upstream declaration is generated from the pinned headers. */
  const void* (*system_thunks)(void);
  /* These return retained references. current_runner returns NULL outside a
     native base sequence. Returned references must be runner_release'd. */
  MojoRsTaskRunner* (*dispatch_runner)(void);
  MojoRsTaskRunner* (*current_runner)(void);
  void (*runner_retain)(MojoRsTaskRunner* runner);
  void (*runner_release)(MojoRsTaskRunner* runner);
  uint32_t (*runner_is_current)(const MojoRsTaskRunner* runner);
  /* Valid callbacks transfer context ownership immediately, including failure.
     Exactly one of run/cancel is called, on any thread; never both. A failure
     can call cancel synchronously. Both callbacks must be non-NULL and must
     consume context without unwinding across C. */
  MojoRsStatus (*runner_post)(MojoRsTaskRunner* runner,
                             void* context,
                             MojoRsTaskCallback run,
                             MojoRsTaskCallback cancel);
  int64_t (*time_ticks_now_micros)(void);
  uint64_t (*next_trace_id)(void);
  /* Parent must be a broker. On success the child owns an inherited endpoint
     encoded in --mojo-rust-sdk-bootstrap=TOKEN. Success means launched and
     invitation sent; the typed SDK handshake must confirm acceptance. */
  MojoRsStatus (*spawn_child)(const MojoRsSpawnOptions* options,
                             MojoRsHandle* bootstrap_pipe,
                             MojoRsChild** child);
  /* Child calls after non-broker runtime_init. May block: do not run on the
     native dispatch or IO sequence or an async executor worker. */
  MojoRsStatus (*accept_inherited)(const char* token,
                                  MojoRsHandle* bootstrap_pipe);
  MojoRsStatus (*child_wait)(MojoRsChild* child,
                            uint64_t timeout_ms,
                            int32_t* exit_code);
  /* Releases the process handle; does not terminate the child. */
  void (*child_release)(MojoRsChild* child);
  /* Single-connection named listener. Windows uses a named pipe, Linux a
     Unix-domain socket, macOS a Mach bootstrap service name. Namespace policy
     can require launchd MachServices deployment. name must be nonempty.
     Listener creation never takes over the host loop. */
  MojoRsStatus (*listen_once)(const char* name, MojoRsListener** listener);
  /* Accept/send invitation, potentially blocking until a peer connects.
     peer_pid must identify the client on Windows/macOS for native-handle
     transfer. Zero is supported only on Linux. */
  MojoRsStatus (*listener_accept)(MojoRsListener* listener,
                                 uint64_t peer_pid,
                                 uint64_t timeout_ms,
                                 MojoRsHandle* bootstrap_pipe);
  void (*listener_release)(MojoRsListener* listener);
  /* Connects and accepts an invitation. Runs blocking I/O. */
  MojoRsStatus (*connect_named)(const char* name,
                               MojoRsHandle* bootstrap_pipe);
} MojoRsApiV1;

/* Copy out an exact v1 table. Caller supplies sizeof(MojoRsApiV1). No API field
   may be read before this succeeds. An incompatible struct size is rejected. */
MOJO_RS_EXPORT MojoRsStatus MojoRsGetApiV1(uint32_t abi_version,
                                         uint32_t out_size,
                                         MojoRsApiV1* out);

#ifdef __cplusplus
}  /* extern "C" */
#endif
#endif
