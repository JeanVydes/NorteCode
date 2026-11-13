//! The signals that ask the interface to stop.
//!
//! The terminal has to be restored before the process leaves, and a signal
//! ends the process without giving the interface the chance to do it. The
//! signal is therefore watched in its own thread, and the run loop stops on
//! its own terms.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

/// A flag that is set when the process is asked to stop.
pub(crate) struct StopSignal {
    asked: Arc<AtomicBool>,
}

impl StopSignal {
    /// Starts watching for the signals that ask the process to stop.
    pub(crate) fn listen() -> Self {
        let asked = Arc::new(AtomicBool::new(false));
        let watched = Arc::clone(&asked);
        std::thread::spawn(move || {
            let Ok(runtime) = tokio::runtime::Runtime::new() else {
                return;
            };
            runtime.block_on(wait_for_signal());
            watched.store(true, Ordering::SeqCst);
        });
        Self { asked }
    }

    /// Whether the process was asked to stop.
    pub(crate) fn was_asked(&self) -> bool {
        self.asked.load(Ordering::SeqCst)
    }
}

/// Waits for the signal that asks the process to stop.
async fn wait_for_signal() {
    let interrupt = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut signal) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            signal.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        () = interrupt => {}
        () = terminate => {}
    }
}
