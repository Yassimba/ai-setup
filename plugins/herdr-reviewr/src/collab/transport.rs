//! The local collaboration socket: reviewr listens, the Pi extension connects.
//!
//! One `interprocess` local socket (Unix domain socket / Windows named pipe) per review
//! session, named deterministically from the user and canonical worktree so the extension
//! can find it without discovery. Frames are newline-delimited JSON; parsing stays in
//! [`super::protocol`] — this module only moves lines. The listener accepts one live
//! connection at a time; a newer connection replaces the old (a restarted Pi reconnects),
//! and every event is tagged with its connection id so a stale reader's close can never be
//! mistaken for the live link going down.

use std::io::{BufRead, BufReader, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};

use interprocess::TryClone as _;
use interprocess::local_socket::traits::{Listener as _, Stream as _};
use interprocess::local_socket::{GenericFilePath, ListenerOptions, Stream, ToFsName};

/// The deterministic socket path for one worktree: both sides derive it independently —
/// reviewr from its resolved repo root, the extension from `git rev-parse --show-toplevel`
/// — so no side channel is needed to meet. The hash input is the shared
/// [`super::context::canonical_worktree_key`] normalization, so a Windows verbatim
/// `\\?\C:\...` path and Node's plain `C:\...` land on the same pipe. An explicit filesystem
/// path (Unix socket file / Windows named pipe) rather than a namespaced name, because the
/// Node extension must be able to compute the identical address. The user rides in the hash
/// because Linux's `/tmp` is system-global; the temp dir itself is already per-user on macOS.
pub fn socket_path(worktree: &std::path::Path) -> String {
    let canonical = super::context::canonical_worktree_key(worktree);
    let user = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "anon".to_string());
    let hash = super::materialize::key_hash(&format!("{user}|{canonical}"));
    if cfg!(windows) {
        format!(r"\\.\pipe\reviewr-collab-{hash}")
    } else {
        std::env::temp_dir()
            .join(format!("reviewr-collab-{hash}.sock"))
            .to_string_lossy()
            .into_owned()
    }
}

/// A Deep Review session's socket, derived from its target key rather than the worktree:
/// a local-target deep pane shares its worktree with the origin sidebar (which already owns
/// the worktree-derived socket), so the pair is pinned to this address via the pane
/// environment instead.
pub fn socket_path_for_key(target_key: &str) -> String {
    let user = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "anon".to_string());
    let hash = super::materialize::key_hash(&format!("{user}|deep|{target_key}"));
    if cfg!(windows) {
        format!(r"\\.\pipe\reviewr-collab-{hash}")
    } else {
        std::env::temp_dir()
            .join(format!("reviewr-collab-{hash}.sock"))
            .to_string_lossy()
            .into_owned()
    }
}

/// One transport event, tagged with the connection that produced it.
#[derive(Debug)]
pub enum TransportEvent {
    Connected { conn: u64 },
    Line { conn: u64, line: String },
    Closed { conn: u64 },
}

/// The live write half shared between the accept loop and `send`.
type Shared = Arc<Mutex<Option<(u64, Stream)>>>;

/// The listening transport. Dropping it leaves the listener thread parked on `accept` —
/// harmless for the app's lifetime model, where the transport lives as long as the process.
#[derive(Debug)]
pub struct CollabTransport {
    events: mpsc::Receiver<TransportEvent>,
    current: Shared,
}

impl CollabTransport {
    /// Bind the socket and start accepting. `None` when another live reviewr already
    /// serves this worktree or the platform refused the address — collaboration degrades
    /// to off; reviewing continues. A stale Unix socket file left by a crash is reclaimed:
    /// if nothing answers on it, it is removed and the bind retried once.
    pub fn bind(path: &str) -> Option<Self> {
        let fs_name = path.to_fs_name::<GenericFilePath>().ok()?;
        let listener = match ListenerOptions::new().name(fs_name.clone()).create_sync() {
            Ok(listener) => listener,
            Err(error) if error.kind() == std::io::ErrorKind::AddrInUse && !cfg!(windows) => {
                if Stream::connect(fs_name).is_ok() {
                    return None; // a live server owns it
                }
                let _ = std::fs::remove_file(path);
                let retry = path.to_fs_name::<GenericFilePath>().ok()?;
                ListenerOptions::new().name(retry).create_sync().ok()?
            }
            Err(_) => return None,
        };
        let (tx, rx) = mpsc::channel();
        let current: Shared = Arc::new(Mutex::new(None));
        let slot = Arc::clone(&current);
        std::thread::spawn(move || {
            let ids = AtomicU64::new(1);
            loop {
                let Ok(stream) = listener.accept() else { break };
                let conn = ids.fetch_add(1, Ordering::Relaxed);
                let Ok(reader) = stream.try_clone() else { continue };
                if let Ok(mut slot) = slot.lock() {
                    *slot = Some((conn, stream));
                }
                if tx.send(TransportEvent::Connected { conn }).is_err() {
                    break;
                }
                let tx = tx.clone();
                let slot = Arc::clone(&slot);
                std::thread::spawn(move || {
                    let mut lines = BufReader::new(reader).lines();
                    while let Some(Ok(line)) = lines.next() {
                        if tx.send(TransportEvent::Line { conn, line }).is_err() {
                            return;
                        }
                    }
                    // Only clear the slot if this connection still owns it — a newer
                    // connection may have replaced it while this reader drained.
                    if let Ok(mut slot) = slot.lock()
                        && slot.as_ref().is_some_and(|(id, _)| *id == conn)
                    {
                        *slot = None;
                    }
                    let _ = tx.send(TransportEvent::Closed { conn });
                });
            }
        });
        Some(Self { events: rx, current })
    }

    /// Drain everything the socket produced since the last frame, non-blocking.
    pub fn drain(&self) -> Vec<TransportEvent> {
        let mut events = Vec::new();
        while let Ok(event) = self.events.try_recv() {
            events.push(event);
        }
        events
    }

    /// The live connection id, when one is up.
    pub fn live_conn(&self) -> Option<u64> {
        self.current.lock().ok().and_then(|slot| slot.as_ref().map(|(id, _)| *id))
    }

    /// Write one frame line to the live connection. A write failure is quiet here — the
    /// reader thread reports the close authoritatively.
    pub fn send(&self, line: &str) {
        if let Ok(mut slot) = self.current.lock()
            && let Some((_, stream)) = slot.as_mut()
        {
            let _ = stream.write_all(line.as_bytes());
            let _ = stream.write_all(b"\n");
            let _ = stream.flush();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socket_paths_are_deterministic_and_distinct_per_worktree() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        assert_eq!(socket_path(a.path()), socket_path(a.path()));
        assert_ne!(socket_path(a.path()), socket_path(b.path()));
        assert!(socket_path(a.path()).contains("reviewr-collab-"));
    }

    #[test]
    fn lines_flow_in_and_out_and_a_reconnect_replaces_the_live_connection() {
        let dir = tempfile::tempdir().unwrap();
        let path = socket_path(dir.path());
        let transport = CollabTransport::bind(&path).expect("bind");
        let fs = path.as_str().to_fs_name::<GenericFilePath>().unwrap();

        let mut first = Stream::connect(fs.clone()).expect("connect");
        first.write_all(b"{\"v\":1}\n").unwrap();
        first.flush().unwrap();

        // Poll until the background threads deliver both events.
        let mut got_line = false;
        let mut conn_id = 0;
        for _ in 0..200 {
            for event in transport.drain() {
                match event {
                    TransportEvent::Connected { conn } => conn_id = conn,
                    TransportEvent::Line { line, .. } => {
                        assert_eq!(line, "{\"v\":1}");
                        got_line = true;
                    }
                    TransportEvent::Closed { .. } => {}
                }
            }
            if got_line {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(got_line, "the inbound line arrived");
        assert_eq!(transport.live_conn(), Some(conn_id));

        // Outbound reaches the client.
        transport.send("{\"type\":\"hello_ack\"}");
        let mut reply = String::new();
        BufReader::new(&mut first).read_line(&mut reply).unwrap();
        assert_eq!(reply.trim(), "{\"type\":\"hello_ack\"}");

        // A second connection replaces the first as the live writer.
        let _second = Stream::connect(fs).expect("reconnect");
        for _ in 0..200 {
            if transport.live_conn() != Some(conn_id) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_ne!(transport.live_conn(), Some(conn_id), "the newer connection owns the slot");
    }
}
