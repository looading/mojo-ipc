# Explicit Windows 7 API profile

`native/scripts/build_native.py --target-os win --target-cpu x64 --windows7`
creates an experimental Win7 SP1 x64 API profile. The default runtime and default
official Chromium pin are unchanged; custom Chromium still requires an explicit
matching revision. A Chromium pin is not itself a Windows compatibility claim.

The profile removes the normal console subsystem configuration and links PE OS
and subsystem versions6.1. Module-local x64 import-pointer definitions resolve
GetSystemTimePreciseAsFileTime to GetSystemTimeAsFileTime. SetThreadInformation
returns FALSE/ERROR_NOT_SUPPORTED because memory/EcoQoS hints are optional and
unavailable; it never reports a successful change. The explicit profile also adapts Chromium base and BoringSSL entropy to the
existing BCryptGenRandom system-preferred CNG backend, failing closed on error.
It never falls back to weak randomness or requires bcryptprimitives on Win7.
This profile runs in ordinary desktop processes, not a restricted Chromium sandbox.
The managed adapter records source hashes; default modern builds retain the original
entropy behavior. IPC, allocator, process and
shutdown code remain the native Chromium implementation. Wallclock precision is
lower; interval measurements continue to use the existing monotonic clock.

The native smoke checks the actual compatibility functions and reports the
RtlGetVersion host tuple. Cross-building never executes Windows code or marks
native_smoke_passed=true. The manifest records windows_api_profile=win7-sp1,
minimum_windows_build=7601 and win7_runtime_verified=false until a separate
acceptance receipt verifies the actual Win7 SP1 kernel. A Windows10 smoke run
is useful evidence for this profile but cannot substitute for Win7 execution.
Audit normal and delay imports for every library and smoke executable, and run
Rust parent/child IPC plus application collection/reporting acceptance on the
intended target before claiming OS support. No general Chromium Win7 support or
Windows x86 support is implied by this option.
