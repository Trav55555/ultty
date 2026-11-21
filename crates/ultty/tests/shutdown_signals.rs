//! The termination signals reach the shutdown callback, once.
//!
//! This lives in a test binary of its own, as one test: the handlers cannot
//! be removed again, and in the shared unit-test binary they would leave
//! Ctrl-C and SIGTERM doing nothing for every test that runs after.

use std::os::unix::net::UnixListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

use nix::sys::signal::{raise, signal, SigHandler, Signal};
use ultty::control::auth::write_auth_token;
use ultty::control::paths::ControlEndpoint;
use ultty::control::ControlServer;
use ultty::platform::shutdown::on_shutdown_signal;

#[test]
fn termination_signals_start_shutdown_once() {
    let dir = tempfile::tempdir().unwrap();
    let mut dead_child = std::process::Command::new("true").spawn().unwrap();
    dead_child.wait().unwrap();
    let stale = ControlEndpoint::for_pid(dir.path(), dead_child.id());
    drop(UnixListener::bind(&stale.socket).unwrap());
    std::fs::write(&stale.token, "stale").unwrap();

    let server = ControlServer::new_in(dir.path()).unwrap();
    write_auth_token(server.token_path()).unwrap();
    let socket_path = server.socket_path().to_path_buf();
    let token_path = server.token_path().to_path_buf();
    assert!(!stale.socket.exists());
    assert!(!stale.token.exists());
    assert!(socket_path.exists());
    assert!(token_path.exists());

    // As under `nohup`.
    // SAFETY: SIG_IGN runs no code in signal context.
    unsafe { signal(Signal::SIGHUP, SigHandler::SigIgn) }.unwrap();

    let calls = Arc::new(AtomicUsize::new(0));
    let (tx, rx) = mpsc::channel();
    let counted = Arc::clone(&calls);
    on_shutdown_signal(move || {
        counted.fetch_add(1, Ordering::SeqCst);
        tx.send(()).unwrap();
    })
    .unwrap();

    // An inherited ignore is kept.
    raise(Signal::SIGHUP).unwrap();
    assert!(
        rx.recv_timeout(Duration::from_millis(300)).is_err(),
        "an ignored SIGHUP should not start shutdown"
    );

    // SIGTERM starts shutdown, and the signals after it neither run the
    // callback again nor kill the process.
    raise(Signal::SIGTERM).unwrap();
    rx.recv_timeout(Duration::from_secs(5))
        .expect("SIGTERM should start shutdown");
    drop(server);
    assert!(!socket_path.exists());
    assert!(!token_path.exists());
    raise(Signal::SIGINT).unwrap();
    raise(Signal::SIGTERM).unwrap();
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}
