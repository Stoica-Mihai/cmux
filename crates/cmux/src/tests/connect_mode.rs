use super::*;
use cmux_proto::{ProbeKind, SessionStatus};
use std::sync::atomic::AtomicU64;
use std::time::{Duration, Instant};

fn info(id: u64) -> SessionInfo {
    SessionInfo {
        id,
        label: format!("s{id}"),
        cwd: std::path::PathBuf::from("/tmp"),
        cmd: vec!["bash".into()],
        probe: ProbeKind::None,
        rows: 24,
        cols: 80,
        spawned_at_ms: 0,
        last_active_ms: 0,
        status: SessionStatus::Unknown,
        attention: false,
        alive: true,
        exit_status: None,
    }
}

fn slot() -> DaemonSlot {
    DaemonSlot {
        parser: Arc::new(Mutex::new(crate::session::TerminalState::fresh(24, 80))),
        byte_ring: Arc::new(Mutex::new(VecDeque::new())),
        dirty: Arc::new(AtomicBool::new(false)),
        alive: Arc::new(AtomicBool::new(true)),
        last_active_ms: Arc::new(AtomicU64::new(0)),
        pending_status: Arc::new(Mutex::new(None)),
        exit_status: Arc::new(Mutex::new(None)),
    }
}

fn handle() -> (DaemonHandle, mpsc::Receiver<Request>) {
    let (req_tx, req_rx) = mpsc::channel();
    let h = DaemonHandle {
        req_tx,
        slots: Default::default(),
        pending_spawns: Default::default(),
        alive: Arc::new(AtomicBool::new(true)),
    };
    (h, req_rx)
}

#[test]
fn a_fulfilled_mailbox_hands_over_the_info() {
    let mb = SpawnMailbox::new();
    mb.fulfill(info(7));
    let got = mb
        .wait(200)
        .expect("a fulfilled mailbox should hand over its info");
    assert_eq!(got.id, 7, "the mailbox handed over the wrong session");
}

#[test]
fn a_mailbox_delivers_its_value_exactly_once() {
    let mb = SpawnMailbox::new();
    mb.fulfill(info(3));
    assert!(
        mb.wait(200).is_some(),
        "the first wait should take the info"
    );
    assert!(
        mb.wait(20).is_none(),
        "the info was delivered twice; each SpawnSession must match one waiter"
    );
}

#[test]
fn an_empty_mailbox_waits_out_its_timeout() {
    let mb = SpawnMailbox::new();
    let timeout_ms = 120;
    let started = Instant::now();
    let got = mb.wait(timeout_ms);
    let elapsed = started.elapsed();

    assert!(got.is_none(), "an empty mailbox returned a session");
    assert!(
        elapsed >= Duration::from_millis(timeout_ms),
        "wait returned after {elapsed:?}, short of its {timeout_ms}ms timeout"
    );
}

#[test]
fn a_mailbox_hands_over_across_threads() {
    let mb = SpawnMailbox::new();
    let writer = mb.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(30));
        writer.fulfill(info(11));
    });

    let started = Instant::now();
    let got = mb
        .wait(5_000)
        .expect("the reader thread never saw the info");
    let elapsed = started.elapsed();

    assert_eq!(got.id, 11, "the wrong session crossed the thread boundary");
    assert!(
        elapsed < Duration::from_millis(2_000),
        "wait took {elapsed:?}, so it timed out rather than waking on the notify"
    );
}

#[test]
fn a_second_waiter_on_a_taken_mailbox_times_out() {
    let mb = SpawnMailbox::new();
    mb.fulfill(info(5));
    assert!(
        mb.wait(200).is_some(),
        "the first wait should take the info"
    );

    let other = mb.clone();
    let joined = std::thread::spawn(move || other.wait(80))
        .join()
        .expect("waiter thread");
    assert!(
        joined.is_none(),
        "a second thread took an already-delivered session"
    );
}

#[test]
fn request_reaches_the_writer_channel() {
    let (h, rx) = handle();
    h.request(Request::ListSessions).expect("send");
    let got = rx
        .try_recv()
        .expect("the request never reached the channel");
    assert!(
        matches!(got, Request::ListSessions),
        "the channel received {got:?}"
    );
}

#[test]
fn request_errors_once_the_writer_thread_is_gone() {
    let (h, rx) = handle();
    drop(rx);
    let err = h
        .request(Request::Shutdown)
        .expect_err("a closed channel should surface as an error, not a panic");
    assert!(
        err.to_string().contains("daemon channel closed"),
        "a dead writer channel reported {err}"
    );
}

#[test]
fn a_registered_slot_is_found_by_its_remote_id() {
    let (h, _rx) = handle();
    h.register_slot(42, slot());

    let slots = h.slots.lock().expect("slots");
    assert!(
        slots.contains_key(&42),
        "the slot registered under 42 is not in the map"
    );
    assert!(
        !slots.contains_key(&43),
        "an unregistered id resolved to a slot"
    );
}

#[test]
fn registering_the_same_id_twice_keeps_the_newer_slot() {
    let (h, _rx) = handle();
    h.register_slot(1, slot());
    let second = slot();
    let marker = second.dirty.clone();
    h.register_slot(1, second);

    let stored = h
        .slots
        .lock()
        .expect("slots")
        .get(&1)
        .cloned()
        .expect("slot 1");
    stored.dirty.store(true, Ordering::SeqCst);
    assert!(
        marker.load(Ordering::SeqCst),
        "id 1 still resolves to the first slot, so daemon frames would land on a stale grid"
    );
}

/// The other direction: a slot dropped before its session was wired up must
/// stop resolving, or a FrameDelta lands on a session with no row.
#[test]
fn a_forgotten_slot_stops_resolving() {
    let (h, _rx) = handle();
    h.register_slot(9, slot());
    h.forget_slot(9);
    assert!(
        !h.slots.lock().expect("slots").contains_key(&9),
        "the forgotten slot is still registered"
    );
}
