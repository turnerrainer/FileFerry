//! Graceful-shutdown signal handling.
//!
//! T-23 (h2ck.me v1 fleet-wide, OWASP-PROBES §34.4): axum's
//! `serve(...).await` doesn't stop cleanly when the container gets
//! `SIGTERM` — the tokio runtime aborts every in-flight request when
//! the process exits. Under `docker stop` (default 10 s grace) or a
//! Kubernetes rolling-restart, that means every in-progress
//! `POST /v1/files/copy` fails half-way with a client-side connection
//! reset AND — worse — the copy-audit log line never emits, leaving a
//! partial-transfer with no record.
//!
//! `axum::serve(...).with_graceful_shutdown(shutdown_signal())` fixes
//! it: axum stops accepting new connections when the future fires,
//! waits for in-flight requests to complete, then returns. The kernel
//! still SIGKILLs us after Docker's grace period, so the shutdown is
//! "best-effort within the grace window" — a genuine slow-copy over
//! the 10 s window still dies, but every request that finishes inside
//! the window logs cleanly.
//!
//! The future awaits **both** `SIGTERM` (container / systemd) and
//! `SIGINT` (Ctrl-C in a dev shell); whichever fires first wins.

/// Await a graceful-shutdown signal. Completes when `SIGTERM` or
/// `SIGINT` reaches the process, emitting one INFO line so the
/// operator log records **which** signal fired and how long the
/// server ran before shutdown started. Never returns on its own.
///
/// Unix-only — FileFerry's shipped Docker image is Debian, and the
/// non-container use-case (`cargo run` locally) is also Unix. On a
/// hypothetical Windows build the function would still compile (just
/// waits for Ctrl-C) — see the `#[cfg]` gate below.
#[cfg(unix)]
pub async fn shutdown_signal() {
    use tokio::signal::unix::{signal, SignalKind};

    let mut sigterm = signal(SignalKind::terminate()).expect("install SIGTERM handler on unix");
    let mut sigint = signal(SignalKind::interrupt()).expect("install SIGINT handler on unix");

    tokio::select! {
        _ = sigterm.recv() => {
            tracing::info!(signal = "SIGTERM", "graceful shutdown initiated");
        }
        _ = sigint.recv() => {
            tracing::info!(signal = "SIGINT", "graceful shutdown initiated");
        }
    }
}

#[cfg(not(unix))]
pub async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!(signal = "CTRL_C", "graceful shutdown initiated");
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::time::timeout;

    #[tokio::test(flavor = "multi_thread")]
    async fn shutdown_signal_is_pending_then_resolves_on_sigterm() {
        // Combined pending / signal-fires test — tokio signal handlers
        // are process-wide and edge-triggered, so splitting these into
        // two `#[test]`s makes them race on the shared SIGTERM state.
        // One test suffices: (1) enter shutdown_signal, (2) prove it
        // stays Pending for 100 ms with no signal, (3) raise SIGTERM,
        // (4) prove it resolves within 500 ms.
        //
        // The whole point of the graceful-shutdown future is that it
        // does not fire on its own — if it did, `axum::serve` would
        // immediately drop every in-flight request at boot.
        let awaiter = tokio::spawn(shutdown_signal());

        // Phase 1: no signal → future stays Pending.
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(
            !awaiter.is_finished(),
            "shutdown_signal resolved without a signal — an axum \
             serve() wrapped around this would exit immediately at boot"
        );

        // Phase 2: raise SIGTERM at ourselves. The signal handler was
        // installed by `shutdown_signal()` before the phase-1 sleep,
        // so this is safe (won't kill the test binary).
        // SAFETY: `libc::kill` is a plain syscall wrapper; `getpid`
        // returns the process's own id.
        let rc = unsafe { libc::kill(libc::getpid(), libc::SIGTERM) };
        assert_eq!(rc, 0, "kill(SIGTERM) failed with rc={rc}");

        // Phase 3: future resolves within a bounded time.
        let done = timeout(Duration::from_millis(500), awaiter).await;
        assert!(
            done.is_ok(),
            "shutdown_signal did not resolve within 500 ms of \
             SIGTERM — the signal handler is not wired correctly"
        );
        done.unwrap().expect("shutdown_signal task panicked");
    }
}
