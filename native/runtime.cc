// Native boundary for the pinned Chromium Mojo implementation.
// The process owns this runtime until exit. There is deliberately no Core
// shutdown or library-unload operation in v1.
#include "include/mojo_rs_native.h"

#include <atomic>
#include <chrono>
#include <condition_variable>
#include <limits>
#include <memory>
#include <mutex>
#include <string>
#include <tuple>
#include <utility>

#include "base/at_exit.h"
#include "base/command_line.h"
#include "base/compiler_specific.h"
#include "base/feature_list.h"
#include "base/files/file_path.h"
#include "base/functional/bind.h"
#include "base/location.h"
#include "base/memory/raw_ptr_exclusion.h"
#include "base/memory/scoped_refptr.h"
#include "base/process/launch.h"
#include "base/process/process.h"
#include "base/task/single_thread_task_runner.h"
#include "base/threading/thread.h"
#include "base/time/time.h"
#include "base/trace_event/trace_id_helper.h"
#include "build/build_config.h"
#include "mojo/core/embedder/configuration.h"
#include "mojo/core/embedder/embedder.h"
#include "mojo/core/embedder/scoped_ipc_support.h"
#include "mojo/public/c/system/core.h"
#include "mojo/public/c/system/thunks.h"
#include "mojo/public/cpp/platform/named_platform_channel.h"
#include "mojo/public/cpp/platform/platform_channel.h"
#include "mojo/public/cpp/platform/platform_channel_server.h"
#include "mojo/public/cpp/platform/platform_handle.h"

struct MojoRsTaskRunner {
  explicit MojoRsTaskRunner(
      scoped_refptr<base::SingleThreadTaskRunner> task_runner)
      : runner(std::move(task_runner)) {}
  std::atomic<uint32_t> references{1};
  scoped_refptr<base::SingleThreadTaskRunner> runner;
};

struct MojoRsChild {
  explicit MojoRsChild(base::Process value) : process(std::move(value)) {}
  base::Process process;
};

namespace {

constexpr char kCommit[] = MOJO_RS_CHROMIUM_COMMIT;
constexpr char kBootstrapSwitch[] = "mojo-rust-sdk-bootstrap";
constexpr char kBootstrapName[] = "mojo-rust-sdk/bootstrap/v1";

struct RuntimeState {
  std::mutex init_mutex;
  bool attempted = false;
  bool broker = false;
  std::atomic<bool> initialized{false};
  std::unique_ptr<base::AtExitManager> exit_manager;
  std::unique_ptr<base::Thread> io_thread;
  std::unique_ptr<base::Thread> dispatch_thread;
  std::unique_ptr<mojo::core::ScopedIPCSupport> ipc_support;
};

RuntimeState& Runtime() {
  // The API table, Core singleton, task runners, and AtExitManager must not be
  // destroyed while generated bindings or native callbacks can still exist.
  // Process teardown reclaims them. Runtime acquisition is not Core ownership.
  static RuntimeState* const state = new RuntimeState;
  return *state;
}

bool Ready() {
  return Runtime().initialized.load(std::memory_order_acquire);
}

bool IsNativeWorker() {
  if (!Ready()) {
    return false;
  }
  return Runtime().io_thread->task_runner()->BelongsToCurrentThread() ||
         Runtime().dispatch_thread->task_runner()->BelongsToCurrentThread();
}

MojoRsStatus RuntimeInit(const MojoRsInitOptions* options) {
  if (!options || options->struct_size < sizeof(*options) ||
      options->is_broker > 1) {
    return MOJO_RESULT_INVALID_ARGUMENT;
  }
  auto& state = Runtime();
  std::lock_guard lock(state.init_mutex);
  const bool broker = options->is_broker != 0;
  if (state.attempted) {
    return Ready() && state.broker == broker ? MOJO_RESULT_OK
                                           : MOJO_RESULT_FAILED_PRECONDITION;
  }
  state.attempted = true;
  state.broker = broker;
  state.exit_manager = std::make_unique<base::AtExitManager>();
  if (!base::CommandLine::InitializedForCurrentProcess()) {
    base::CommandLine::Init(0, nullptr);
  }
  if (!base::FeatureList::GetInstance()) {
    base::FeatureList::InitInstance("", "");
  }
  mojo::core::InitFeatures();
  mojo::core::Configuration config;
  config.is_broker_process = broker;
  mojo::core::Init(config);

  state.io_thread = std::make_unique<base::Thread>("mojo-rs-io");
  if (!state.io_thread->StartWithOptions(
          base::Thread::Options(base::MessagePumpType::IO, 0)) ||
      !state.io_thread->WaitUntilThreadStarted()) {
    return MOJO_RESULT_UNAVAILABLE;
  }
  // ShutdownPolicy has no effect for this revision's ipcz Core. This object
  // only installs the I/O runner; v1 does not claim automatic drain/flush.
  state.ipc_support = std::make_unique<mojo::core::ScopedIPCSupport>(
      state.io_thread->task_runner(),
      mojo::core::ScopedIPCSupport::ShutdownPolicy::FAST);
  state.dispatch_thread = std::make_unique<base::Thread>("mojo-rs-dispatch");
  if (!state.dispatch_thread->Start() ||
      !state.dispatch_thread->WaitUntilThreadStarted()) {
    return MOJO_RESULT_UNAVAILABLE;
  }
  state.initialized.store(true, std::memory_order_release);
  return MOJO_RESULT_OK;
}

const void* SystemThunks() {
  return Ready() ? MojoEmbedderGetSystemThunks2() : nullptr;
}

MojoRsTaskRunner* DispatchRunner() {
  return Ready()
             ? new MojoRsTaskRunner(Runtime().dispatch_thread->task_runner())
             : nullptr;
}

MojoRsTaskRunner* CurrentRunner() {
  if (!Ready() || !base::SingleThreadTaskRunner::HasCurrentDefault()) {
    return nullptr;
  }
  return new MojoRsTaskRunner(
      base::SingleThreadTaskRunner::GetCurrentDefault());
}

void RunnerRetain(MojoRsTaskRunner* runner) {
  if (runner) {
    runner->references.fetch_add(1, std::memory_order_relaxed);
  }
}

void RunnerRelease(MojoRsTaskRunner* runner) {
  if (runner &&
      runner->references.fetch_sub(1, std::memory_order_acq_rel) == 1) {
    delete runner;
  }
}

uint32_t RunnerIsCurrent(const MojoRsTaskRunner* runner) {
  return runner && runner->runner->BelongsToCurrentThread();
}

class PostedTask {
 public:
  PostedTask(void* context,
             MojoRsTaskCallback run,
             MojoRsTaskCallback cancel)
      : context_(context), run_(run), cancel_(cancel) {}
  ~PostedTask() {
    if (pending_.exchange(false, std::memory_order_acq_rel)) {
      cancel_(context_);
    }
  }
  void Run() {
    if (pending_.exchange(false, std::memory_order_acq_rel)) {
      run_(context_);
    }
  }

 private:
  std::atomic<bool> pending_{true};
  // Opaque callback-owned memory can come from the Rust host's allocator. Its
  // ownership is consumed exactly once by run_ or cancel_, outside native PA.
  RAW_PTR_EXCLUSION void* const context_;
  const MojoRsTaskCallback run_;
  const MojoRsTaskCallback cancel_;
};

MojoRsStatus RunnerPost(MojoRsTaskRunner* runner,
                       void* context,
                       MojoRsTaskCallback run,
                       MojoRsTaskCallback cancel) {
  if (!run || !cancel) {
    // Invalid callback pairs do not transfer context ownership.
    return MOJO_RESULT_INVALID_ARGUMENT;
  }
  auto task = std::make_shared<PostedTask>(context, run, cancel);
  if (!runner) {
    return MOJO_RESULT_INVALID_ARGUMENT;  // task destructor invokes cancel.
  }
  const bool accepted = runner->runner->PostTask(
      FROM_HERE, base::BindOnce(
                     [](std::shared_ptr<PostedTask> posted) { posted->Run(); },
                     std::move(task)));
  // PostTask destroys the closure on rejection. PostedTask's destructor then
  // calls cancel exactly once. Accepted closures call run exactly once.
  return accepted ? MOJO_RESULT_OK : MOJO_RESULT_UNAVAILABLE;
}

int64_t TimeTicksNowMicros() {
  return base::TimeTicks::Now().since_origin().InMicroseconds();
}

uint64_t NextTraceId() {
  return base::trace_event::GetNextGlobalTraceId();
}

void MakeTransportEndpoint(mojo::PlatformChannelEndpoint endpoint,
                           MojoPlatformHandle* handle,
                           MojoInvitationTransportEndpoint* transport) {
  mojo::PlatformHandle::ToMojoPlatformHandle(endpoint.TakePlatformHandle(),
                                            handle);
  transport->struct_size = sizeof(*transport);
  transport->type = MOJO_INVITATION_TRANSPORT_TYPE_CHANNEL;
  transport->num_platform_handles = 1;
  transport->platform_handles = handle;
}

void MakeProcessHandle(const base::Process& process,
                       MojoPlatformProcessHandle* handle) {
  handle->struct_size = sizeof(*handle);
#if BUILDFLAG(IS_WIN)
  handle->value =
      static_cast<uint64_t>(reinterpret_cast<uintptr_t>(process.Handle()));
#else
  handle->value = static_cast<uint64_t>(process.Handle());
#endif
}

MojoRsStatus SendBootstrap(mojo::PlatformChannelEndpoint endpoint,
                          const base::Process& process,
                          MojoRsHandle* out_pipe) {
  if (!endpoint.is_valid()) {
    return MOJO_RESULT_INVALID_ARGUMENT;
  }
  MojoHandle invitation = MOJO_HANDLE_INVALID;
  MojoHandle pipe = MOJO_HANDLE_INVALID;
  MojoResult result = MojoCreateInvitation(nullptr, &invitation);
  if (result != MOJO_RESULT_OK) {
    return result;
  }
  result = MojoAttachMessagePipeToInvitation(
      invitation, kBootstrapName, sizeof(kBootstrapName) - 1, nullptr, &pipe);
  if (result != MOJO_RESULT_OK) {
    std::ignore = MojoClose(invitation);
    return result;
  }
  MojoPlatformHandle handle{};
  MojoInvitationTransportEndpoint transport{};
  MakeTransportEndpoint(std::move(endpoint), &handle, &transport);
  MojoPlatformProcessHandle process_handle{};
  const MojoPlatformProcessHandle* process_ptr = nullptr;
  if (process.IsValid()) {
    MakeProcessHandle(process, &process_handle);
    process_ptr = &process_handle;
  }
  result = MojoSendInvitation(invitation, process_ptr, &transport, nullptr, 0,
                              nullptr);
  if (result != MOJO_RESULT_OK) {
    // This pinned ipcz implementation adopts the platform endpoint during the
    // attempt, before ConnectNode can fail. Do not re-adopt/close the raw value
    // here: it may already be closed or owned by a Transport. This follows the
    // same revision's C++ invitation wrapper rather than assuming that every
    // non-OK result leaves the platform handle with the caller.
    std::ignore = MojoClose(invitation);
    std::ignore = MojoClose(pipe);
    return result;
  }
  *out_pipe = pipe;
  return MOJO_RESULT_OK;
}

MojoRsStatus AcceptBootstrap(mojo::PlatformChannelEndpoint endpoint,
                            MojoRsHandle* out_pipe) {
  if (!endpoint.is_valid()) {
    return MOJO_RESULT_UNAVAILABLE;
  }
  MojoPlatformHandle handle{};
  MojoInvitationTransportEndpoint transport{};
  MakeTransportEndpoint(std::move(endpoint), &handle, &transport);
  MojoHandle invitation = MOJO_HANDLE_INVALID;
  const MojoResult result =
      MojoAcceptInvitation(&transport, nullptr, &invitation);
  if (result != MOJO_RESULT_OK) {
    // As above, the complete transport is handed to the pinned Core for the
    // attempt; recovering the numeric handle here could double-close it.
    return result;
  }
  const MojoResult extracted = MojoExtractMessagePipeFromInvitation(
      invitation, kBootstrapName, sizeof(kBootstrapName) - 1, nullptr, out_pipe);
  std::ignore = MojoClose(invitation);
  return extracted;
}

MojoRsStatus SpawnChild(const MojoRsSpawnOptions* options,
                       MojoRsHandle* bootstrap_pipe,
                       MojoRsChild** child) {
  if (!options || options->struct_size < sizeof(*options) || !options->program ||
      !*options->program || (!options->args && options->args_len != 0) ||
      !bootstrap_pipe || !child) {
    return MOJO_RESULT_INVALID_ARGUMENT;
  }
  *bootstrap_pipe = MOJO_HANDLE_INVALID;
  *child = nullptr;
  if (!Ready() || !Runtime().broker || IsNativeWorker()) {
    return MOJO_RESULT_FAILED_PRECONDITION;
  }
  base::CommandLine command(
      base::FilePath::FromUTF8Unsafe(options->program));
  for (size_t i = 0; i < options->args_len; ++i) {
    // SAFETY: The C ABI caller supplies an args_len-element array. Nonempty
    // arrays were checked for null above, and i is below that supplied length.
    const char* argument = UNSAFE_BUFFERS(options->args[i]);
    if (!argument) {
      return MOJO_RESULT_INVALID_ARGUMENT;
    }
    command.AppendArg(argument);
  }
  mojo::PlatformChannel channel;
  base::LaunchOptions launch_options;
  const std::string token =
      channel.PrepareToPassRemoteEndpoint(launch_options);
  command.AppendSwitchASCII(kBootstrapSwitch, token);
  base::Process process = base::LaunchProcess(command, launch_options);
  channel.RemoteProcessLaunchAttempted();
  if (!process.IsValid()) {
    return MOJO_RESULT_UNAVAILABLE;
  }
  const MojoRsStatus sent = SendBootstrap(channel.TakeLocalEndpoint(), process,
                                          bootstrap_pipe);
  if (sent != MOJO_RESULT_OK) {
    // A child which cannot receive its required bootstrap must not be left
    // waiting forever by this convenience spawn operation.
    std::ignore = process.Terminate(1, true);
    return sent;
  }
  *child = new MojoRsChild(std::move(process));
  return MOJO_RESULT_OK;
}

MojoRsStatus AcceptInherited(const char* token, MojoRsHandle* bootstrap_pipe) {
  if (!token || !*token || !bootstrap_pipe) {
    return MOJO_RESULT_INVALID_ARGUMENT;
  }
  *bootstrap_pipe = MOJO_HANDLE_INVALID;
  if (!Ready() || Runtime().broker || IsNativeWorker()) {
    return MOJO_RESULT_FAILED_PRECONDITION;
  }
  return AcceptBootstrap(
      mojo::PlatformChannel::RecoverPassedEndpointFromString(token),
      bootstrap_pipe);
}

MojoRsStatus ChildWait(MojoRsChild* child,
                      uint64_t timeout_ms,
                      int32_t* exit_code) {
  if (!child || !exit_code ||
      timeout_ms > static_cast<uint64_t>(std::numeric_limits<int64_t>::max())) {
    return MOJO_RESULT_INVALID_ARGUMENT;
  }
  if (IsNativeWorker()) {
    return MOJO_RESULT_FAILED_PRECONDITION;
  }
  int code = 0;
  if (!child->process.WaitForExitWithTimeout(
          base::Milliseconds(static_cast<int64_t>(timeout_ms)), &code)) {
    return MOJO_RESULT_DEADLINE_EXCEEDED;
  }
  *exit_code = code;
  return MOJO_RESULT_OK;
}

void ChildRelease(MojoRsChild* child) {
  delete child;
}

struct ListenerState {
  std::mutex mutex;
  std::condition_variable changed;
  bool started = false;
  bool start_ok = false;
  bool connected = false;
  bool cancelled = false;
  bool consumed = false;
  mojo::PlatformChannelEndpoint endpoint;
  // Created, used, and destroyed only on the native I/O runner.
  std::unique_ptr<mojo::PlatformChannelServer> server;
};

void CancelListener(const std::shared_ptr<ListenerState>& state) {
  {
    std::lock_guard lock(state->mutex);
    state->cancelled = true;
  }
  state->changed.notify_all();
  const bool posted = Runtime().io_thread->task_runner()->PostTask(
      FROM_HERE, base::BindOnce(
                     [](std::shared_ptr<ListenerState> listener) {
                       listener->server.reset();
                     },
                     state));
  if (!posted) {
    // No further callback is possible when the runner rejects new work, but
    // its server still must not be destroyed on this caller's sequence.
    // This path can occur only during process teardown in the v1 lifecycle.
    std::ignore = new std::shared_ptr<ListenerState>(state);
  }
}

}  // namespace

struct MojoRsListener {
  std::shared_ptr<ListenerState> state;
};

namespace {

MojoRsStatus ListenOnce(const char* name, MojoRsListener** listener) {
  if (!name || !*name || !listener) {
    return MOJO_RESULT_INVALID_ARGUMENT;
  }
  *listener = nullptr;
  if (!Ready() || !Runtime().broker || IsNativeWorker()) {
    return MOJO_RESULT_FAILED_PRECONDITION;
  }
  mojo::NamedPlatformChannel::Options options;
  options.server_name = mojo::NamedPlatformChannel::ServerNameFromUTF8(name);
  mojo::NamedPlatformChannel channel(options);
  auto server_endpoint = channel.TakeServerEndpoint();
  if (!server_endpoint.is_valid()) {
    return MOJO_RESULT_UNAVAILABLE;
  }
  auto state = std::make_shared<ListenerState>();
  const bool posted = Runtime().io_thread->task_runner()->PostTask(
      FROM_HERE,
      base::BindOnce(
          [](std::shared_ptr<ListenerState> state,
             mojo::PlatformChannelServerEndpoint endpoint) {
            state->server = std::make_unique<mojo::PlatformChannelServer>();
            const bool ok = state->server->Listen(
                std::move(endpoint),
                base::BindOnce(
                    [](std::shared_ptr<ListenerState> state,
                       mojo::PlatformChannelEndpoint endpoint) {
                      {
                        std::lock_guard lock(state->mutex);
                        state->connected = true;
                        state->endpoint = std::move(endpoint);
                      }
                      state->changed.notify_all();
                    },
                    state));
            {
              std::lock_guard lock(state->mutex);
              state->start_ok = ok;
              state->started = true;
            }
            state->changed.notify_all();
          },
          state, std::move(server_endpoint)));
  if (!posted) {
    return MOJO_RESULT_UNAVAILABLE;
  }
  std::unique_lock lock(state->mutex);
  state->changed.wait(lock, [&] { return state->started; });
  if (!state->start_ok) {
    lock.unlock();
    CancelListener(state);
    return MOJO_RESULT_UNAVAILABLE;
  }
  *listener = new MojoRsListener{state};
  return MOJO_RESULT_OK;
}

MojoRsStatus ListenerAccept(MojoRsListener* listener,
                           uint64_t peer_pid,
                           uint64_t timeout_ms,
                           MojoRsHandle* bootstrap_pipe) {
  if (!listener || !bootstrap_pipe ||
      timeout_ms > static_cast<uint64_t>(std::numeric_limits<int64_t>::max()) ||
      peer_pid >
          static_cast<uint64_t>(std::numeric_limits<base::ProcessId>::max())) {
    return MOJO_RESULT_INVALID_ARGUMENT;
  }
  *bootstrap_pipe = MOJO_HANDLE_INVALID;
  if (!Ready() || !Runtime().broker || IsNativeWorker()) {
    return MOJO_RESULT_FAILED_PRECONDITION;
  }
#if BUILDFLAG(IS_WIN) || BUILDFLAG(IS_MAC)
  if (peer_pid == 0) {
    return MOJO_RESULT_INVALID_ARGUMENT;
  }
#endif
  base::Process peer;
  if (peer_pid != 0) {
    // In particular, Windows Mojo needs PROCESS_DUP_HANDLE privileges for
    // transferring shared buffers/handles. This API is for trusted SDK peers.
    peer = base::Process::OpenWithExtraPrivileges(
        static_cast<base::ProcessId>(peer_pid));
    if (!peer.IsValid()) {
      return MOJO_RESULT_PERMISSION_DENIED;
    }
  }
  auto state = listener->state;
  std::unique_lock lock(state->mutex);
  if (state->consumed) {
    return MOJO_RESULT_FAILED_PRECONDITION;
  }
  const bool awakened = state->changed.wait_for(
      lock, std::chrono::milliseconds(static_cast<int64_t>(timeout_ms)),
      [&] { return state->connected || state->cancelled; });
  if (!awakened) {
    return MOJO_RESULT_DEADLINE_EXCEEDED;
  }
  if (state->cancelled || !state->endpoint.is_valid()) {
    return MOJO_RESULT_CANCELLED;
  }
  state->consumed = true;
  auto endpoint = std::move(state->endpoint);
  lock.unlock();
  return SendBootstrap(std::move(endpoint), peer, bootstrap_pipe);
}

void ListenerRelease(MojoRsListener* listener) {
  if (listener) {
    CancelListener(listener->state);
    delete listener;
  }
}

MojoRsStatus ConnectNamed(const char* name, MojoRsHandle* bootstrap_pipe) {
  if (!name || !*name || !bootstrap_pipe) {
    return MOJO_RESULT_INVALID_ARGUMENT;
  }
  *bootstrap_pipe = MOJO_HANDLE_INVALID;
  if (!Ready() || Runtime().broker || IsNativeWorker()) {
    return MOJO_RESULT_FAILED_PRECONDITION;
  }
  return AcceptBootstrap(mojo::NamedPlatformChannel::ConnectToServer(
                             mojo::NamedPlatformChannel::ServerNameFromUTF8(name)),
                         bootstrap_pipe);
}

const MojoRsApiV1 kApi = {
    sizeof(MojoRsApiV1),
    1,
    kCommit,
    RuntimeInit,
    SystemThunks,
    DispatchRunner,
    CurrentRunner,
    RunnerRetain,
    RunnerRelease,
    RunnerIsCurrent,
    RunnerPost,
    TimeTicksNowMicros,
    NextTraceId,
    SpawnChild,
    AcceptInherited,
    ChildWait,
    ChildRelease,
    ListenOnce,
    ListenerAccept,
    ListenerRelease,
    ConnectNamed,
};

}  // namespace

extern "C" MOJO_RS_EXPORT MojoRsStatus MojoRsGetApiV1(uint32_t abi_version,
                                                       uint32_t out_size,
                                                       MojoRsApiV1* out) {
  if (abi_version != 1 || out_size != sizeof(MojoRsApiV1) || !out) {
    return MOJO_RESULT_INVALID_ARGUMENT;
  }
  *out = kApi;
  return MOJO_RESULT_OK;
}
