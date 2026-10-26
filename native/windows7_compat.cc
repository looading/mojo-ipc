// Win7 API 配置：显式替换本模块对两项 Win8 API 的导入。
// 高级线程提示返回真实的“不支持”，不伪造成功；UTC 时钟使用系统旧 API。
#include "windows7_compat.h"
#include <windows.h>
#include <cstdint>
#include <cstdio>

static_assert(sizeof(void*) == 8, "Win7 profile currently supports x64 only");

namespace {
void WINAPI LegacyWallclock(LPFILETIME output) {
  ::GetSystemTimeAsFileTime(output);
}
BOOL WINAPI UnsupportedThreadHint(HANDLE, THREAD_INFORMATION_CLASS, LPVOID, DWORD) {
  ::SetLastError(ERROR_NOT_SUPPORTED);
  return FALSE;
}
uint64_t Ticks(const FILETIME& value) {
  return (uint64_t{value.dwHighDateTime} << 32) | value.dwLowDateTime;
}
}  // namespace

// Windows x64 dllimport 调用引用这些指针槽；模块内强符号阻止链接器从
// Kernel32 import library 引入 Win8 的同名导入。只对显式 Win7 产物编译。
extern "C" {
extern decltype(&::GetSystemTimePreciseAsFileTime) __imp_GetSystemTimePreciseAsFileTime;
extern decltype(&::SetThreadInformation) __imp_SetThreadInformation;
decltype(&::GetSystemTimePreciseAsFileTime) __imp_GetSystemTimePreciseAsFileTime = &LegacyWallclock;
decltype(&::SetThreadInformation) __imp_SetThreadInformation = &UnsupportedThreadHint;
}

bool MojoRsWindows7CompatSmoke() {
  using VersionFn = LONG (WINAPI*)(OSVERSIONINFOEXW*);
  auto version_fn = reinterpret_cast<VersionFn>(::GetProcAddress(::GetModuleHandleW(L"ntdll.dll"), "RtlGetVersion"));
  OSVERSIONINFOEXW version{};
  version.dwOSVersionInfoSize = sizeof(version);
  if (!version_fn || version_fn(&version) != 0) return false;
  std::printf("windows native host: major=%lu minor=%lu build=%lu service_pack=%u\n",
              version.dwMajorVersion, version.dwMinorVersion, version.dwBuildNumber,
              static_cast<unsigned>(version.wServicePackMajor));

  FILETIME before{}, current{}, after{};
  ::GetSystemTimeAsFileTime(&before);
  ::GetSystemTimePreciseAsFileTime(&current);
  ::GetSystemTimeAsFileTime(&after);
  ::SetLastError(ERROR_SUCCESS);
  MEMORY_PRIORITY_INFORMATION hint{MEMORY_PRIORITY_NORMAL};
  const BOOL changed = ::SetThreadInformation(::GetCurrentThread(), ThreadMemoryPriority, &hint, sizeof(hint));
  return Ticks(current) >= Ticks(before) && Ticks(current) <= Ticks(after) &&
         !changed && ::GetLastError() == ERROR_NOT_SUPPORTED;
}
