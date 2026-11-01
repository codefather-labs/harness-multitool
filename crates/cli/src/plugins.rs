//! A local build's own plugins (CON-2). Add a crate that implements an interface to this binary
//! and list it here; the settings' Callbacks tab then shows it.

use harness_daemon::SharedHandler;

/// The custom callback handlers of this build (IF-16), run by the daemon.
pub fn callback_handlers() -> Vec<SharedHandler> {
    // vec![std::sync::Arc::new(my_crate::MyHandler::new())]
    Vec::new()
}
