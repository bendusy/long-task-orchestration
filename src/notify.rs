//! Cross-process wake transport for the self-driving wake loop (Phase 2 of
//! references/self-driving-wake-loop.md).
//!
//! A waiter (`lto events --wait`) binds a localhost TCP listener on an
//! auto-assigned port and registers it under `.lto/<run-id>/notify-endpoints.json`.
//! When a runner finishes and `lto agent-turn-completed` writes the event, it
//! calls `wake_run`, which connects-and-closes to every registered port. The
//! bare connect unblocks the waiter's accept loop near-instantly, so it can
//! re-check events.jsonl without waiting out its full poll interval.
//!
//! Pure std on purpose: LTO sets `unsafe_code = "forbid"`, so we cannot use the
//! `nix::poll`/`BorrowedFd::borrow_raw` approach hcom uses. Instead the listener
//! is non-blocking and the waiter polls `accept()` between sleeps — the same
//! pattern hcom's own tests use.

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Single-target connect-drop timeout. If the connect misses, the waiter still
/// catches the event on its next poll tick — wake is an optimization, not a
/// correctness requirement.
const WAKE_CONNECT_TIMEOUT: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, Serialize, Deserialize)]
struct NotifyEndpoint {
    waiter_id: String,
    port: u16,
}

fn endpoints_path(repo: &Path, run_id: &str) -> PathBuf {
    repo.join(".lto").join(run_id).join("notify-endpoints.json")
}

/// A registered wake listener. Drop removes its endpoint entry.
pub struct NotifyServer {
    listener: TcpListener,
    port: u16,
    repo: PathBuf,
    run_id: String,
    waiter_id: String,
}

impl NotifyServer {
    /// Bind a localhost listener on an auto-assigned port and register it for
    /// the run so wakers can find it.
    pub fn register(repo: &Path, run_id: &str, waiter_id: &str) -> anyhow::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        listener.set_nonblocking(true)?;
        let server = Self {
            listener,
            port,
            repo: repo.to_path_buf(),
            run_id: run_id.to_string(),
            waiter_id: waiter_id.to_string(),
        };
        server.add_endpoint()?;
        Ok(server)
    }

    /// Drain any pending wake connections without blocking. Returns true if at
    /// least one connect was accepted since the last drain (i.e. we were woken).
    pub fn drain(&self) -> bool {
        let mut woken = false;
        loop {
            match self.listener.accept() {
                Ok((stream, _)) => {
                    drop(stream);
                    woken = true;
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(_) => break,
            }
        }
        woken
    }

    fn add_endpoint(&self) -> anyhow::Result<()> {
        let waiter_id = self.waiter_id.clone();
        let port = self.port;
        with_endpoints_locked(&self.repo, &self.run_id, |endpoints| {
            // Replace any stale entry for the same waiter_id, then append ours.
            endpoints.retain(|e| e.waiter_id != waiter_id);
            endpoints.push(NotifyEndpoint { waiter_id, port });
            true
        })
    }

    fn remove_endpoint(&self) {
        let waiter_id = self.waiter_id.clone();
        let _ = with_endpoints_locked(&self.repo, &self.run_id, |endpoints| {
            let before = endpoints.len();
            endpoints.retain(|e| e.waiter_id != waiter_id);
            endpoints.len() != before
        });
    }
}

/// Run a read-modify-write on the endpoints file under an exclusive file lock so
/// concurrent waiters cannot lose each other's entries (audit #1). The closure
/// mutates the endpoint list and returns whether a write-back is needed. The
/// write itself is atomic (temp file + rename) so a concurrent `wake_run`
/// reader never observes a half-written file.
fn with_endpoints_locked(
    repo: &Path,
    run_id: &str,
    edit: impl FnOnce(&mut Vec<NotifyEndpoint>) -> bool,
) -> anyhow::Result<()> {
    let path = endpoints_path(repo, run_id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let lock_path = path.with_extension("json.lock");
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)?;
    lock.lock_exclusive()?;
    // Unlock via Drop, not a trailing call: `edit` is caller-supplied and a
    // panic inside it would skip a manual unlock and strand the lock file for
    // the rest of the process.
    let _guard = FlockGuard(&lock);
    let mut endpoints = read_endpoints(&path);
    if edit(&mut endpoints) {
        write_endpoints_atomic(&path, &endpoints)?;
    }
    Ok(())
}

struct FlockGuard<'a>(&'a std::fs::File);

impl Drop for FlockGuard<'_> {
    fn drop(&mut self) {
        let _ = FileExt::unlock(self.0);
    }
}

impl Drop for NotifyServer {
    fn drop(&mut self) {
        self.remove_endpoint();
    }
}

/// Wake every registered waiter for a run via connect-and-close. Best-effort:
/// failures (dead/stale endpoints) are ignored — the waiter's poll loop is the
/// correctness backstop.
pub fn wake_run(repo: &Path, run_id: &str) {
    let path = endpoints_path(repo, run_id);
    for endpoint in read_endpoints(&path) {
        if endpoint.port == 0 {
            continue;
        }
        if let Ok(addr) = format!("127.0.0.1:{}", endpoint.port).parse() {
            let _ = TcpStream::connect_timeout(&addr, WAKE_CONNECT_TIMEOUT);
        }
    }
}

fn read_endpoints(path: &Path) -> Vec<NotifyEndpoint> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    match serde_json::from_str::<Value>(&text) {
        Ok(value) => serde_json::from_value(value).unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

/// Atomic write: serialize to a temp file in the same dir, then rename over the
/// target. A concurrent reader sees either the old or the new file, never a
/// truncated/half-written one.
fn write_endpoints_atomic(path: &Path, endpoints: &[NotifyEndpoint]) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(endpoints)?;
    let tmp = path.with_extension(format!("json.tmp.{}", std::process::id()));
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wake_unblocks_a_registered_server() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        let server = NotifyServer::register(repo, "r1", "waiter-a").unwrap();
        // Nothing yet -> drain sees no connection.
        assert!(!server.drain());
        // After a wake, drain reports we were woken.
        wake_run(repo, "r1");
        // Give the connect a moment to land on the listener.
        std::thread::sleep(Duration::from_millis(20));
        assert!(server.drain());
    }

    #[test]
    fn endpoint_is_registered_and_removed_on_drop() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        let path = endpoints_path(repo, "r1");
        {
            let _server = NotifyServer::register(repo, "r1", "waiter-a").unwrap();
            assert_eq!(read_endpoints(&path).len(), 1);
        }
        // Drop removed the endpoint.
        assert_eq!(read_endpoints(&path).len(), 0);
    }

    #[test]
    fn wake_run_is_safe_with_no_endpoints() {
        let tmp = tempfile::tempdir().unwrap();
        // No endpoints file at all -> no panic, no error.
        wake_run(tmp.path(), "missing-run");
    }

    #[test]
    fn stale_endpoint_for_same_waiter_is_replaced() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        let path = endpoints_path(repo, "r1");
        let _a = NotifyServer::register(repo, "r1", "waiter-a").unwrap();
        let _b = NotifyServer::register(repo, "r1", "waiter-b").unwrap();
        assert_eq!(read_endpoints(&path).len(), 2);
        // Re-registering waiter-a replaces, not duplicates.
        let _a2 = NotifyServer::register(repo, "r1", "waiter-a").unwrap();
        assert_eq!(read_endpoints(&path).len(), 2);
    }

    #[test]
    fn concurrent_registers_do_not_lose_endpoints() {
        // Audit #1: without the file lock, concurrent read-modify-write would
        // lose endpoints. Register N waiters from N threads and assert all land.
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().to_path_buf();
        let n = 8;
        let handles: Vec<_> = (0..n)
            .map(|i| {
                let repo = repo.clone();
                std::thread::spawn(move || {
                    // Keep the server alive past registration so its endpoint
                    // persists for the final count (Drop would remove it).
                    NotifyServer::register(&repo, "r1", &format!("waiter-{i}")).unwrap()
                })
            })
            .collect();
        let _servers: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        let path = endpoints_path(&repo, "r1");
        assert_eq!(
            read_endpoints(&path).len(),
            n,
            "all concurrent registrations must survive the file lock"
        );
    }
}
