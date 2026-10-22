// Executable integration test. Build through this overlay's runtime_smoke GN
// target; it links only the SDK ABI and upstream C type declarations.
#include "mojo_rs_native.h"

#include <atomic>
#include <chrono>
#include <condition_variable>
#include <cstring>
#include <fstream>
#include <iostream>
#include <mutex>
#include <span>
#include <string>
#include <string_view>
#include <thread>
#include <tuple>
#include <vector>

#include "mojo/public/c/system/core.h"
#include "mojo/public/c/system/thunks.h"

namespace {

MojoRsApiV1 api{};
const MojoSystemThunks2* mojo = nullptr;

bool Check(bool condition, const char* message) {
  if (!condition) {
    std::cerr << "FAIL: " << message << '\n';
  }
  return condition;
}

bool Initialize(bool broker) {
  const MojoRsInitOptions options{sizeof(options), broker ? 1u : 0u};
  if (!Check(api.runtime_init(&options) == MOJO_RESULT_OK, "runtime_init")) {
    return false;
  }
  mojo = static_cast<const MojoSystemThunks2*>(api.system_thunks());
  return Check(mojo && mojo->size >= sizeof(*mojo), "system thunk table");
}

bool Send(MojoHandle endpoint,
          std::string_view bytes,
          const std::vector<MojoHandle>& handles = {}) {
  MojoMessageHandle message = 0;
  if (mojo->CreateMessage(nullptr, &message) != MOJO_RESULT_OK) {
    return false;
  }
  MojoAppendMessageDataOptions options{};
  options.struct_size = sizeof(options);
  options.flags = MOJO_APPEND_MESSAGE_DATA_FLAG_COMMIT_SIZE;
  void* buffer = nullptr;
  uint32_t capacity = 0;
  const auto appended = mojo->AppendMessageData(
      message, static_cast<uint32_t>(bytes.size()), handles.data(),
      static_cast<uint32_t>(handles.size()), &options, &buffer, &capacity);
  if (appended != MOJO_RESULT_OK) {
    std::ignore = mojo->DestroyMessage(message);
    return false;
  }
  if (capacity < bytes.size() || (!buffer && !bytes.empty())) {
    std::ignore = mojo->DestroyMessage(message);
    return false;
  }
  if (!bytes.empty()) {
    // SAFETY: AppendMessageData returned a writable buffer with the capacity
    // checked above, and bytes is a valid view of exactly bytes.size() bytes.
#pragma clang unsafe_buffer_usage begin
    std::memcpy(buffer, bytes.data(), bytes.size());
#pragma clang unsafe_buffer_usage end
  }
  // The write consumes the message even when the peer closes concurrently.
  return mojo->WriteMessage(endpoint, message, nullptr) == MOJO_RESULT_OK;
}

bool Receive(MojoHandle endpoint,
             std::string* bytes,
             std::vector<MojoHandle>* handles) {
  const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(5);
  MojoMessageHandle message = 0;
  for (;;) {
    const auto result = mojo->ReadMessage(endpoint, nullptr, &message);
    if (result == MOJO_RESULT_OK) {
      break;
    }
    if (result != MOJO_RESULT_SHOULD_WAIT ||
        std::chrono::steady_clock::now() >= deadline) {
      return false;
    }
    std::this_thread::sleep_for(std::chrono::milliseconds(1));
  }
  void* buffer = nullptr;
  uint32_t num_bytes = 0;
  uint32_t num_handles = 0;
  auto result = mojo->GetMessageData(message, nullptr, &buffer, &num_bytes,
                                    nullptr, &num_handles);
  if (result != MOJO_RESULT_OK && result != MOJO_RESULT_RESOURCE_EXHAUSTED) {
    std::ignore = mojo->DestroyMessage(message);
    return false;
  }
  handles->resize(num_handles);
  result = mojo->GetMessageData(message, nullptr, &buffer, &num_bytes,
                                handles->data(), &num_handles);
  if (result == MOJO_RESULT_OK) {
    bytes->assign(static_cast<const char*>(buffer), num_bytes);
  }
  std::ignore = mojo->DestroyMessage(message);
  return result == MOJO_RESULT_OK;
}

bool WaitForPeerClosed(MojoHandle endpoint) {
  const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(5);
  for (;;) {
    MojoHandleSignalsState state{};
    if (mojo->QueryHandleSignalsState(endpoint, &state) != MOJO_RESULT_OK) {
      return false;
    }
    if (state.satisfied_signals & MOJO_HANDLE_SIGNAL_PEER_CLOSED) {
      return true;
    }
    if (std::chrono::steady_clock::now() >= deadline) {
      return false;
    }
    std::this_thread::sleep_for(std::chrono::milliseconds(1));
  }
}

bool ChildExchange(MojoRsHandle endpoint) {
  std::string bytes;
  std::vector<MojoHandle> handles;
  if (!Check(Receive(endpoint, &bytes, &handles), "child receive") ||
      !Check(bytes == "request" && handles.size() == 2, "child request handles")) {
    return false;
  }
  void* mapped = nullptr;
  if (!Check(mojo->MapBuffer(handles[1], 0, 4, nullptr, &mapped) == MOJO_RESULT_OK,
             "child map transferred shared buffer")) {
    return false;
  }
  // SAFETY: MapBuffer succeeded for four bytes; both literals contain at least
  // four bytes, and the mapping remains live until UnmapBuffer below.
#pragma clang unsafe_buffer_usage begin
  const bool content_ok = std::memcmp(mapped, "PING", 4) == 0;
  std::memcpy(mapped, "PONG", 4);
#pragma clang unsafe_buffer_usage end
  std::ignore = mojo->UnmapBuffer(mapped);
  const bool sent = Send(handles[0], "pipe-ok") && Send(endpoint, "response");
  // WriteMessage does not promise that the native I/O thread has flushed its
  // transport. Keep this process alive until the peer observed both replies.
  std::string acknowledgement;
  std::vector<MojoHandle> acknowledgement_handles;
  const bool acknowledged =
      sent && Receive(endpoint, &acknowledgement, &acknowledgement_handles) &&
      acknowledgement == "ack" && acknowledgement_handles.empty();
  std::ignore = mojo->Close(handles[0]);
  std::ignore = mojo->Close(handles[1]);
  std::ignore = mojo->Close(endpoint);
  return Check(content_ok && sent && acknowledged,
               "child shared bytes, responses and acknowledgement");
}

bool ParentExchange(MojoRsHandle endpoint) {
  MojoHandle pipe_a = 0;
  MojoHandle pipe_b = 0;
  MojoHandle buffer = 0;
  MojoHandle retained = 0;
  if (!Check(mojo->CreateMessagePipe(nullptr, &pipe_a, &pipe_b) == MOJO_RESULT_OK,
             "create transferred pipe") ||
      !Check(mojo->CreateSharedBuffer(4096, nullptr, &buffer) == MOJO_RESULT_OK,
             "create shared buffer") ||
      !Check(mojo->DuplicateBufferHandle(buffer, nullptr, &retained) == MOJO_RESULT_OK,
             "duplicate shared buffer")) {
    return false;
  }
  void* mapped = nullptr;
  if (!Check(mojo->MapBuffer(retained, 0, 4, nullptr, &mapped) == MOJO_RESULT_OK,
             "parent map shared buffer")) {
    return false;
  }
  // SAFETY: MapBuffer succeeded for four bytes and the literal has four bytes.
#pragma clang unsafe_buffer_usage begin
  std::memcpy(mapped, "PING", 4);
#pragma clang unsafe_buffer_usage end
  std::ignore = mojo->UnmapBuffer(mapped);
  if (!Check(Send(endpoint, "request", {pipe_b, buffer}), "send attached handles")) {
    return false;
  }
  // The successfully appended message now owns pipe_b and buffer.
  std::string bytes;
  std::vector<MojoHandle> handles;
  if (!Check(Receive(pipe_a, &bytes, &handles) && bytes == "pipe-ok", "transferred pipe") ||
      !Check(Receive(endpoint, &bytes, &handles) && bytes == "response", "response") ||
      !Check(mojo->MapBuffer(retained, 0, 4, nullptr, &mapped) == MOJO_RESULT_OK,
             "parent remap shared buffer")) {
    return false;
  }
  // SAFETY: The successful mapping and the literal both contain four bytes.
#pragma clang unsafe_buffer_usage begin
  const bool content_ok = std::memcmp(mapped, "PONG", 4) == 0;
#pragma clang unsafe_buffer_usage end
  std::ignore = mojo->UnmapBuffer(mapped);
  // The client exits only after reading this ACK. Waiting for its closed
  // endpoint keeps the broker alive until that ACK has crossed the transport.
  const bool completed =
      Check(content_ok, "cross-process shared-buffer mutation") &&
      Check(Send(endpoint, "ack"), "acknowledge completed exchange") &&
      Check(WaitForPeerClosed(endpoint), "client closes after acknowledgement");
  std::ignore = mojo->Close(retained);
  std::ignore = mojo->Close(pipe_a);
  std::ignore = mojo->Close(endpoint);
  return completed;
}

struct CallbackState {
  std::mutex mutex;
  std::condition_variable changed;
  bool ran = false;
  bool current = false;
  std::atomic<int> runs{0};
  std::atomic<int> cancellations{0};
};

void Run(void* context) {
  auto* state = static_cast<CallbackState*>(context);
  state->runs.fetch_add(1);
  auto* current = api.current_runner();
  const bool on_runner = current && api.runner_is_current(current);
  api.runner_release(current);
  {
    std::lock_guard lock(state->mutex);
    state->current = on_runner;
    state->ran = true;
    state->changed.notify_all();
  }
}

void Cancel(void* context) {
  static_cast<CallbackState*>(context)->cancellations.fetch_add(1);
}

bool TaskSmoke() {
  CallbackState state;
  auto* runner = api.dispatch_runner();
  if (!Check(runner != nullptr, "dispatch runner") ||
      !Check(api.runner_post(runner, &state, Run, Cancel) == MOJO_RESULT_OK,
             "post native task")) {
    return false;
  }
  api.runner_retain(runner);
  api.runner_release(runner);
  api.runner_release(runner);
  std::unique_lock lock(state.mutex);
  const bool done = state.changed.wait_for(lock, std::chrono::seconds(5),
                                           [&] { return state.ran; });
  lock.unlock();
  if (!Check(done && state.current && state.runs == 1 && state.cancellations == 0,
             "task runs exactly once on its native sequence")) {
    return false;
  }
  CallbackState rejected;
  return Check(api.runner_post(nullptr, &rejected, Run, Cancel) ==
                   MOJO_RESULT_INVALID_ARGUMENT &&
                   rejected.runs == 0 && rejected.cancellations == 1,
               "rejected task cancels exactly once");
}

}  // namespace

int main(int argc, char** argv) {
  if (argc < 1 || argv == nullptr) {
    return 1;
  }
  const std::span<char*> arguments(argv, static_cast<size_t>(argc));
  if (!Check(MojoRsGetApiV1(1, sizeof(api), &api) == MOJO_RESULT_OK, "get API") ||
      !Check(api.abi_version == 1 && api.struct_size == sizeof(api), "API layout") ||
      !Check(MojoRsGetApiV1(2, sizeof(api), &api) == MOJO_RESULT_INVALID_ARGUMENT,
             "reject unsupported ABI")) {
    return 1;
  }
  std::string token;
  std::string named_server;
  std::string named_client;
  std::string peer_pid_file;
  for (size_t i = 1; i < arguments.size(); ++i) {
    constexpr std::string_view prefix = "--mojo-rust-sdk-bootstrap=";
    std::string_view arg(arguments[i]);
    if (arg.starts_with(prefix)) {
      token = arg.substr(prefix.size());
    } else if (arg == "--named-server" && i + 1 < arguments.size()) {
      named_server = arguments[++i];
    } else if (arg == "--named-client" && i + 1 < arguments.size()) {
      named_client = arguments[++i];
    } else if (arg == "--peer-pid-file" && i + 1 < arguments.size()) {
      peer_pid_file = arguments[++i];
    }
  }
  if (!named_client.empty()) {
    MojoRsHandle bootstrap = 0;
    return Initialize(false) &&
                   Check(api.connect_named(named_client.c_str(), &bootstrap) == MOJO_RESULT_OK,
                         "connect named channel") &&
                   ChildExchange(bootstrap)
               ? 0
               : 1;
  }
  if (!named_server.empty()) {
    if (!Initialize(true)) {
      return 1;
    }
    MojoRsListener* listener = nullptr;
    if (!Check(api.listen_once(named_server.c_str(), &listener) == MOJO_RESULT_OK,
               "listen named channel")) {
      return 1;
    }
    std::cout << "native named server ready\n" << std::flush;
    uint64_t peer_pid = 0;
    if (peer_pid_file.empty()) {
      std::cin >> peer_pid;
    } else {
      const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(10);
      while (peer_pid == 0 && std::chrono::steady_clock::now() < deadline) {
        std::ifstream input(peer_pid_file);
        input >> peer_pid;
        if (peer_pid == 0) {
          std::this_thread::sleep_for(std::chrono::milliseconds(10));
        }
      }
    }
    MojoRsHandle bootstrap = 0;
    const bool accepted =
        peer_pid != 0 && api.listener_accept(listener, peer_pid, 10000, &bootstrap) ==
                             MOJO_RESULT_OK;
    api.listener_release(listener);
    if (!Check(accepted, "accept independent client") || !ParentExchange(bootstrap)) {
      return 1;
    }
    std::cout << "native named smoke passed\n" << std::flush;
    return 0;
  }
  if (!token.empty()) {
    MojoRsHandle bootstrap = 0;
    return Initialize(false) &&
                   Check(api.accept_inherited(token.c_str(), &bootstrap) == MOJO_RESULT_OK,
                         "accept inherited invitation") &&
                   ChildExchange(bootstrap)
               ? 0
               : 1;
  }
  if (!Initialize(true) || !TaskSmoke()) {
    return 1;
  }
  MojoRsInitOptions repeated{sizeof(repeated), 1};
  MojoRsInitOptions conflicting{sizeof(conflicting), 0};
  if (!Check(api.runtime_init(&repeated) == MOJO_RESULT_OK, "repeat initialization") ||
      !Check(api.runtime_init(&conflicting) == MOJO_RESULT_FAILED_PRECONDITION,
             "reject conflicting role") ||
      !Check(api.next_trace_id() != api.next_trace_id(), "unique trace IDs")) {
    return 1;
  }
  MojoRsSpawnOptions spawn{sizeof(spawn), arguments.front(), nullptr, 0};
  MojoRsHandle bootstrap = 0;
  MojoRsChild* child = nullptr;
  if (!Check(api.spawn_child(&spawn, &bootstrap, &child) == MOJO_RESULT_OK,
             "spawn child with native platform channel")) {
    return 1;
  }
  const bool exchanged = ParentExchange(bootstrap);
  int32_t exit_code = -1;
  const bool exited = api.child_wait(child, 5000, &exit_code) == MOJO_RESULT_OK;
  api.child_release(child);
  if (!exchanged || !Check(exited && exit_code == 0, "child exit")) {
    return 1;
  }
  std::cout << "native runtime smoke passed\n";
  return 0;
}
