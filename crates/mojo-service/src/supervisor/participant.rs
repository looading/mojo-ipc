use mojo_ipc::{Runtime, system::message_pipe::MessageEndpoint};
use std::{
    io,
    sync::atomic::{AtomicBool, Ordering},
    thread,
};

pub const SUPERVISOR_SERVER_ENV: &str = "MOJO_SERVICE_SERVER";
pub const SUPERVISOR_ATTEMPT_ENV: &str = "MOJO_SERVICE_ATTEMPT";
static WATCHING: AtomicBool = AtomicBool::new(false);

/// Opt in to parent-death cleanup, once per managed participant process, before
/// loading the native runtime. Requires the supervisor environment. This reserves
/// stdin and exits the process on EOF/error; it is not for interactive stdin.
pub fn watch_supervisor() -> io::Result<()> {
    if std::env::var_os(SUPERVISOR_SERVER_ENV).is_none() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "missing supervisor bootstrap environment",
        ));
    }
    if WATCHING.swap(true, Ordering::SeqCst) {
        return Ok(());
    }
    match thread::Builder::new()
        .name("mojo-parent-watch".into())
        .spawn(|| {
            let _ = io::copy(&mut io::stdin().lock(), &mut io::sink());
            std::process::exit(0);
        }) {
        Ok(_) => Ok(()),
        Err(error) => {
            WATCHING.store(false, Ordering::SeqCst);
            Err(error)
        }
    }
}

/// Blocking named bootstrap for a managed participant after Runtime::load.
pub fn connect_to_supervisor(runtime: &Runtime) -> io::Result<MessageEndpoint> {
    let name = std::env::var(SUPERVISOR_SERVER_ENV).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "missing or non-UTF8 supervisor bootstrap name",
        )
    })?;
    runtime.connect(&name).map_err(io::Error::other)
}
