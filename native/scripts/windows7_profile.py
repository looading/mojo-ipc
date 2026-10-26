"""Apply a managed Windows7 entropy adapter for an explicit native profile."""
import hashlib
from pathlib import Path

MARKER = '// MOJO_RS_WIN7_ENTROPY_ADAPTER_V1'
HEADER = '#include "sdk/mojo_rs/native/windows7_profile_config.h"'
OLD_CNG_CHUNK = 'static_cast<SIZE_T>(std::numeric_limits<ULONG>::max())'
NEW_CNG_CHUNK = 'static_cast<decltype(remaining.size())>(std::numeric_limits<ULONG>::max())'
OLD_PRNG = '''decltype(&ProcessPrng) GetProcessPrng() {
  HMODULE hmod = LoadLibraryW(L"bcryptprimitives.dll");
  CHECK(hmod);
  decltype(&ProcessPrng) process_prng_fn =
      reinterpret_cast<decltype(&ProcessPrng)>(
          GetProcAddress(hmod, "ProcessPrng"));
  CHECK(process_prng_fn);
  return process_prng_fn;
}'''
NEW_PRNG = '''#if MOJO_RS_WINDOWS7_ENTROPY_PROFILE
BOOL WINAPI Win7CngProcessPrng(PBYTE output, SIZE_T length) {
  // SAFETY: RandBytesInternal supplies data/size from its valid writable span;
  // this callback remains private to GetProcessPrng.
  auto remaining = UNSAFE_BUFFERS(span<uint8_t>(output, length));
  while (!remaining.empty()) {
    const auto chunk = remaining.first(std::min(
        remaining.size(), static_cast<decltype(remaining.size())>(std::numeric_limits<ULONG>::max())));
    if (::BCryptGenRandom(nullptr, chunk.data(), static_cast<ULONG>(chunk.size()),
                          BCRYPT_USE_SYSTEM_PREFERRED_RNG) < 0) {
      return FALSE;
    }
    remaining = remaining.subspan(chunk.size());
  }
  return TRUE;
}
#endif

decltype(&ProcessPrng) GetProcessPrng() {
#if MOJO_RS_WINDOWS7_ENTROPY_PROFILE
  return &Win7CngProcessPrng;
#else
  HMODULE hmod = LoadLibraryW(L"bcryptprimitives.dll");
  CHECK(hmod);
  decltype(&ProcessPrng) process_prng_fn =
      reinterpret_cast<decltype(&ProcessPrng)>(
          GetProcAddress(hmod, "ProcessPrng"));
  CHECK(process_prng_fn);
  return process_prng_fn;
#endif
}'''
OLD_CONDITION = '#if WINAPI_FAMILY_PARTITION(WINAPI_PARTITION_APP) && \\\n    !WINAPI_FAMILY_PARTITION(WINAPI_PARTITION_DESKTOP)'
NEW_CONDITION = '#if MOJO_RS_WINDOWS7_ENTROPY_PROFILE || (WINAPI_FAMILY_PARTITION(WINAPI_PARTITION_APP) && !WINAPI_FAMILY_PARTITION(WINAPI_PARTITION_DESKTOP))'
MAC_X86_MARKER = '# MOJO_RS_WIN7_MAC_X86_ADAPTER_V1'
MAC_X86_IMPORT = 'import("//sdk/mojo_rs/native/windows7_compat.gni")'
OLD_MAC_X86_GUARD = '''  assert(host_os != "mac" || target_cpu != "x86",
         "Windows cross-builds from Mac must be 64-bit.")'''
NEW_MAC_X86_GUARD = '''  # SDK 显式 Win7 静态运行库仅复用已提供的真实 x86 CRT/SDK；
  # 不开放浏览器的 component/ASan 或其他默认 Mac->Windows x86 构建。
  assert(host_os != "mac" || target_cpu != "x86" ||
             (mojo_rs_windows7_compat && !is_component_build && !is_asan),
         "Windows cross-builds from Mac require x64 or the explicit static Win7 SDK profile.")'''

def apply(checkout: Path, *, allow_mac_x86: bool = False):
    paths = ['base/rand_util_win.cc', 'third_party/boringssl/src/crypto/rand/windows.cc']
    planned = []
    for relative in paths:
        file = checkout / relative
        original = file.read_text(encoding='utf-8')
        if MARKER in original:
            required = 'Win7CngProcessPrng' if relative.startswith('base/') else NEW_CONDITION
            if HEADER not in original or required not in original:
                raise RuntimeError('inconsistent Win7 entropy adapter: ' + relative)
            changed = original
            if relative.startswith('base/'):
                # SIZE_T 与 span::size_type 在 Win32 同宽但并非同一 C++ 类型。
                # 只迁移已知旧片段，保持已有 checkout 的可重复生产。
                if original.count(OLD_CNG_CHUNK) == 1:
                    changed = original.replace(OLD_CNG_CHUNK, NEW_CNG_CHUNK, 1)
                elif original.count(NEW_CNG_CHUNK) != 1:
                    raise RuntimeError('inconsistent Win7 CNG size adapter')
        else:
            if original.count('#include <windows.h>') != 1:
                raise RuntimeError('unsupported Win7 entropy source: ' + relative)
            changed = original.replace('#include <windows.h>', '#include <windows.h>\n' + MARKER + '\n' + HEADER, 1)
            if relative.startswith('base/'):
                if OLD_PRNG not in changed:
                    raise RuntimeError('unsupported Chromium ProcessPrng implementation')
                changed = changed.replace(HEADER, HEADER + '\n#if MOJO_RS_WINDOWS7_ENTROPY_PROFILE\n#include <bcrypt.h>\n#endif', 1).replace(OLD_PRNG, NEW_PRNG, 1)
            else:
                if changed.count(OLD_CONDITION) != 2:
                    raise RuntimeError('unsupported BoringSSL Windows entropy source')
                changed = changed.replace(OLD_CONDITION, NEW_CONDITION)
        planned.append((file, relative, original, changed))
    if allow_mac_x86:
        relative = 'build/win/BUILD.gn'
        file = checkout / relative
        original = file.read_text(encoding='utf-8')
        if MAC_X86_MARKER in original:
            if MAC_X86_IMPORT not in original or NEW_MAC_X86_GUARD not in original:
                raise RuntimeError('inconsistent Chromium Mac/x86 guard adapter')
            changed = original
        else:
            if original.count(OLD_MAC_X86_GUARD) != 1 or original.count('if (is_win) {') != 1:
                raise RuntimeError('unsupported Chromium Mac/x86 guard')
            changed = original.replace('if (is_win) {', MAC_X86_MARKER + '\n' + MAC_X86_IMPORT + '\n\nif (is_win) {', 1)
            changed = changed.replace(OLD_MAC_X86_GUARD, NEW_MAC_X86_GUARD, 1)
        planned.append((file, relative, original, changed))
    # Validate every input before mutating any source.
    for file, relative, original, changed in planned:
        if original != changed:
            file.write_text(changed, encoding='utf-8')
    return [{'path': relative, 'sha256': hashlib.sha256(changed.encode()).hexdigest()}
            for _, relative, _, changed in planned]
