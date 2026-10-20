//! Generate typed Mojo interfaces using the pinned official Chromium toolchain.
//!
//! Call this crate from a dedicated protocol crate's `build.rs`, then include
//! `OUT_DIR/mojom/mod.rs` at that crate's root. Python 3.10 or newer is required;
//! the Python parser, generator, templates, and Python dependencies are bundled.

use std::env;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Chromium source revision used by the bundled tools.
pub const CHROMIUM_REVISION: &str = "bcbc09a9f8e4c3fd3ad2ed9cbad41e63c35ccfdd";

/// Configuration for generating a protocol crate's root modules.
#[derive(Debug, Clone)]
pub struct Config {
    source_root: PathBuf,
    sources: Vec<PathBuf>,
    include_roots: Vec<PathBuf>,
    runtime_crate: String,
    features: Vec<String>,
    python: Option<OsString>,
    output: Option<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            source_root: PathBuf::from("schema"),
            sources: Vec::new(),
            include_roots: Vec::new(),
            runtime_crate: "mojo_ipc".into(),
            features: Vec::new(),
            python: None,
            output: None,
        }
    }
}

impl Config {
    /// Create a generator configuration.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the root for logical Mojom import paths, relative to the Cargo package.
    pub fn source_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.source_root = root.into();
        self
    }

    /// Add an entry schema, relative to `source_root` (imports are discovered).
    pub fn source(mut self, source: impl Into<PathBuf>) -> Self {
        self.sources.push(source.into());
        self
    }

    /// Add entry schemas, relative to `source_root`.
    pub fn sources<I, P>(mut self, sources: I) -> Self
    where
        I: IntoIterator<Item = P>,
        P: Into<PathBuf>,
    {
        self.sources.extend(sources.into_iter().map(Into::into));
        self
    }

    /// Search an additional schema root, relative to the Cargo package.
    ///
    /// Its imported types are generated into this same protocol crate. Shared
    /// schemas must have unique logical import paths and module names.
    pub fn include_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.include_roots.push(root.into());
        self
    }

    /// Set the actual Cargo dependency name of `mojo-ipc`, including a rename.
    pub fn runtime_crate(mut self, name: impl Into<String>) -> Self {
        self.runtime_crate = name.into();
        self
    }

    /// Enable a named Mojom `[EnableIf]` feature.
    pub fn enable_feature(mut self, name: impl Into<String>) -> Self {
        self.features.push(name.into());
        self
    }

    /// Override the Python executable (also configurable through `MOJOM_PYTHON`).
    pub fn python(mut self, executable: impl Into<OsString>) -> Self {
        self.python = Some(executable.into());
        self
    }

    /// Override the output base, normally Cargo's `OUT_DIR`.
    pub fn out_dir(mut self, output: impl Into<PathBuf>) -> Self {
        self.output = Some(output.into());
        self
    }

    /// Parse the import closure and generate Rust modules into `OUT_DIR/mojom`.
    ///
    /// Returns the generated root index. The generated modules must be included
    /// at the root of a dedicated protocol crate, preserving upstream paths.
    pub fn generate(self) -> io::Result<PathBuf> {
        if self.sources.is_empty() {
            return Err(invalid("at least one entry .mojom source is required"));
        }
        let manifest_dir = env::var_os("CARGO_MANIFEST_DIR")
            .map(PathBuf::from)
            .unwrap_or(env::current_dir()?);
        let source_root = absolute(&manifest_dir, &self.source_root).canonicalize()?;
        let include_roots: Vec<PathBuf> = self
            .include_roots
            .iter()
            .map(|root| absolute(&manifest_dir, root).canonicalize())
            .collect::<io::Result<_>>()?;
        let runtime = self
            .runtime_crate
            .trim_start_matches("::")
            .replace('-', "_");
        if runtime.is_empty()
            || !runtime
                .chars()
                .enumerate()
                .all(|(i, c)| c == '_' || c.is_ascii_alphabetic() || (i > 0 && c.is_ascii_digit()))
        {
            return Err(invalid("runtime_crate must be one Rust crate identifier"));
        }
        let output = self
            .output
            .or_else(|| env::var_os("OUT_DIR").map(PathBuf::from))
            .ok_or_else(|| invalid("OUT_DIR is unset; set Config::out_dir explicitly"))?;
        let output = absolute(&manifest_dir, &output).join("mojom");
        fs::create_dir_all(&output)?;
        let output = output.canonicalize()?;
        let mut features = self.features;
        if env::var("CARGO_CFG_TARGET_FAMILY")
            .unwrap_or_default()
            .split(',')
            .any(|v| v == "unix")
        {
            features.push("is_posix".into());
        }
        match env::var("CARGO_CFG_TARGET_OS").unwrap_or_default().as_str() {
            "windows" => features.push("is_win".into()),
            "linux" => features.push("is_linux".into()),
            "android" => features.push("is_android".into()),
            "macos" => features.extend(["is_mac".into(), "is_apple".into()]),
            "ios" => features.extend(["is_ios".into(), "is_apple".into()]),
            "fuchsia" => features.push("is_fuchsia".into()),
            _ => {}
        }
        features.sort();
        features.dedup();
        let configuration = serde_json::json!({
            "source_root": source_root,
            "include_roots": include_roots,
            "sources": self.sources,
            "out_dir": output,
            "runtime_path": format!("::{runtime}"),
            "features": features,
            "chromium_revision": CHROMIUM_REVISION,
        });
        let config_file = output.join("cargo-config.json");
        fs::write(&config_file, serde_json::to_vec_pretty(&configuration)?)?;
        let tools = Path::new(env!("CARGO_MANIFEST_DIR")).join("vendor");
        let python = self
            .python
            .or_else(|| env::var_os("MOJOM_PYTHON"))
            .unwrap_or_else(|| OsString::from(if cfg!(windows) { "python" } else { "python3" }));
        let result = Command::new(python)
            .arg("-B")
            .arg(tools.join("cargo_mojom.py"))
            .arg("--manifest")
            .arg(&config_file)
            .env("PYTHONNOUSERSITE", "1")
            .output()?;
        if !result.status.success() {
            return Err(io::Error::other(format!(
                "official Mojom generation failed ({}):\n{}{}",
                result.status,
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr),
            )));
        }
        let dependencies: Vec<PathBuf> =
            serde_json::from_slice(&fs::read(output.join("dependencies.json"))?)?;
        for path in dependencies {
            println!("cargo:rerun-if-changed={}", path.display());
        }
        println!("cargo:rerun-if-changed={}", source_root.display());
        for path in include_roots {
            println!("cargo:rerun-if-changed={}", path.display());
        }
        println!("cargo:rerun-if-env-changed=MOJOM_PYTHON");
        println!("cargo:rerun-if-env-changed=CARGO_CFG_TARGET_OS");
        println!("cargo:rerun-if-env-changed=CARGO_CFG_TARGET_FAMILY");
        println!("cargo:rustc-env=MOJOM_CHROMIUM_REVISION={CHROMIUM_REVISION}");
        Ok(output.join("mod.rs"))
    }
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

fn absolute(root: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_owned()
    } else {
        root.join(path)
    }
}
