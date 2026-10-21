fn main() -> Result<(), Box<dyn std::error::Error>> {
    for name in ["TARGET", "PROFILE", "OPT_LEVEL"] {
        println!("cargo:rustc-env=BENCH_{name}={}", std::env::var(name)?);
    }
    let rustc =
        std::process::Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
            .arg("--version")
            .output()?;
    if !rustc.status.success() {
        return Err("cannot identify benchmark compiler".into());
    }
    println!(
        "cargo:rustc-env=BENCH_RUSTC={}",
        String::from_utf8(rustc.stdout)?.trim()
    );
    mojom_build::Config::new()
        .source_root("schema")
        .source("bench.mojom")
        .runtime_crate("ipc")
        .generate()?;
    Ok(())
}
