"""Apply a managed Windows7 entropy adapter for an explicit native profile."""
import hashlib
from pathlib import Path

MARKER = '// MOJO_RS_WIN7_ENTROPY_ADAPTER_V1'
HEADER = '#include "sdk/mojo_rs/native/windows7_profile_config.h"'
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
        remaining.size(), static_cast<SIZE_T>(std::numeric_limits<ULONG>::max())));
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

def apply(checkout: Path):
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
    # Validate every input before mutating any source.
    for file, relative, original, changed in planned:
        if original != changed:
            file.write_text(changed, encoding='utf-8')
    return [{'path': relative, 'sha256': hashlib.sha256(changed.encode()).hexdigest()}
            for _, relative, _, changed in planned]
