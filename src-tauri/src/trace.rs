//! Verbose tracing for debug builds — Express-middleware-style visibility into
//! everything aiterm does, and nothing at all in a release build.
//!
//! Why this exists alongside [`crate::diag`]: they answer different questions.
//! `diag!` is a small, always-on journal of *what aiterm did* — a handful of
//! lines a session, safe to ship, and the thing that has actually diagnosed
//! real faults. This is the firehose: every IPC call with its arguments, every
//! function on a hot path with its timing and its intermediate values. Useful
//! while chasing something; far too much to leave running.
//!
//! **Release builds have none of it.** `tracing`'s `release_max_level_off`
//! feature compiles every macro in this crate down to nothing when
//! `debug_assertions` is off, so the events are not merely filtered at runtime —
//! they are not in the binary. The subscriber is only installed under
//! `#[cfg(debug_assertions)]` too.
//!
//! The consequence worth knowing: Matt tests from release RPMs, so **none of
//! this appears in the app he runs day to day** — that is what `diag!` is for.
//! Reach for tracing when reproducing something under `npm run tauri dev`.
//!
//! Filtering works the usual way, via `AITERM_TRACE` (or `RUST_LOG`):
//!
//! ```text
//! AITERM_TRACE=aiterm::sessions=trace,aiterm::pty=info npm run tauri dev
//! ```

/// Install the subscriber. Called once from `run()`; a no-op in release.
#[cfg(debug_assertions)]
pub fn init() {
    use tracing_subscriber::fmt::format::FmtSpan;
    use tracing_subscriber::EnvFilter;

    // Default is chatty on our own crate and quiet on everyone else's — tauri
    // and wry at debug level bury anything of ours.
    let filter = EnvFilter::try_from_env("AITERM_TRACE")
        .or_else(|_| EnvFilter::try_from_default_env())
        .unwrap_or_else(|_| EnvFilter::new("aiterm=debug,warn"));

    // `FmtSpan::CLOSE` is what turns instrumented functions into timings: each
    // span reports its own elapsed time when it ends, which is the difference
    // between "these things happened" and "this is where the time went".
    let done = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_span_events(FmtSpan::CLOSE)
        .with_target(true)
        .with_writer(std::io::stderr)
        .try_init();
    if done.is_ok() {
        tracing::debug!("tracing installed — release builds compile this away");
    }
}

#[cfg(not(debug_assertions))]
pub fn init() {}

/// Wrap the generated IPC handler so every call from the webview is logged
/// before it dispatches — the `morgan`/Express-middleware equivalent.
///
/// `tauri::ipc::Invoke` is documented as "used internally by macros and is
/// explicitly **NOT** stable", so this is the one place in aiterm that leans on
/// an unstable Tauri API. The blast radius is a compile error on a Tauri bump,
/// not a runtime fault, and it is behind `debug_assertions` — a release build
/// never goes through this path at all.
///
/// Arguments are logged as their JSON. That is the point of the middleware, and
/// it means a command carrying something private would log it: today none do
/// (the credential paths take an id and read the secret themselves — see
/// `providers.rs`), and the log is stderr on a dev machine. Worth re-checking if
/// a command is ever given a secret as a parameter.
#[cfg(debug_assertions)]
pub fn log_invokes<R, F>(handler: F) -> impl Fn(tauri::ipc::Invoke<R>) -> bool + Send + Sync + 'static
where
    R: tauri::Runtime,
    F: Fn(tauri::ipc::Invoke<R>) -> bool + Send + Sync + 'static,
{
    move |invoke| {
        let cmd = invoke.message.command().to_string();
        // InvokeBody is either JSON or a raw byte payload; a pty write is the
        // raw kind and its length is all anyone wants to see of it.
        let args = match invoke.message.payload() {
            tauri::ipc::InvokeBody::Json(v) => {
                let s = v.to_string();
                // Long enough for an id and a path, short enough that a
                // transcript-sized argument cannot flood the terminal.
                if s.len() > 300 {
                    format!("{}… ({} bytes)", &s[..300], s.len())
                } else {
                    s
                }
            }
            tauri::ipc::InvokeBody::Raw(bytes) => format!("<{} raw bytes>", bytes.len()),
        };
        tracing::debug!(target: "aiterm::ipc", "→ {cmd} {args}");
        handler(invoke)
    }
}

#[cfg(not(debug_assertions))]
pub fn log_invokes<R, F>(handler: F) -> F
where
    R: tauri::Runtime,
    F: Fn(tauri::ipc::Invoke<R>) -> bool + Send + Sync + 'static,
{
    handler
}
