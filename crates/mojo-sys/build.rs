fn main() {
    println!("cargo:rerun-if-env-changed=MOJO_RUNTIME_LIBRARY");
    // The runtime is loaded explicitly. No Chromium C++ symbols, archives,
    // rpaths or build-machine library directories are injected into the host.
}
