//! Running futures on every target.
//!
//! # Responsibilities
//!
//! - Run a future to completion on the calling thread on native
//!   ([`block_on`]).
//! - Start a future that returns nothing ([`spawn`]): it blocks until the
//!   future finishes on native, and runs on the browser's event loop on the
//!   web.
//!
//! # Design
//!
//! On the web, a future that waits for something from the browser (a timer,
//! a network fetch) only finishes after the browser's event loop gets control
//! back. Blocking on such a future stops that loop, so it would never finish;
//! that is why [`block_on`] exists only on native. Code that needs a future's
//! value on the web must `.await` it instead.

// Standard library
use std::future::Future;

/// Runs `future` to completion on the calling thread and returns its output.
///
/// This function exists only on native. On the web, a future has to be awaited
/// or passed to [`spawn`].
///
/// # Panics
///
/// Panics if `future` panics.
#[cfg(not(target_arch = "wasm32"))]
pub fn block_on<F: Future>(future: F) -> F::Output {
    pollster::block_on(future)
}

/// Starts `future` and returns nothing.
///
/// On native it runs to completion before this returns; on the web it runs on
/// the browser's event loop after the current task yields.
///
/// # Panics
///
/// On native, panics if `future` panics. On the web, the browser reports the
/// panic as an uncaught error instead.
pub fn spawn<F: Future<Output = ()> + 'static>(future: F) {
    #[cfg(not(target_arch = "wasm32"))]
    pollster::block_on(future);
    #[cfg(target_arch = "wasm32")]
    wasm_bindgen_futures::spawn_local(future);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_on_returns_the_output() {
        assert_eq!(block_on(async { 7 }), 7);
    }

    #[test]
    fn spawn_runs_the_future_on_native() {
        let did_run = std::rc::Rc::new(std::cell::Cell::new(false));
        let did_run_in_future = did_run.clone();
        spawn(async move { did_run_in_future.set(true) });
        assert!(did_run.get());
    }
}
