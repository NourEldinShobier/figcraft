//! A second guard for `figcraft-cli mcp` over stdio. The server already stops when its stdin closes,
//! but a client that dies while a grandchild still holds the pipe open leaves the server running
//! for good. This watches the parent process and exits once it is gone.
//!
//! Set `FIGCRAFT_MCP_NO_PARENT_WATCH=1` for launchers that exit right after starting the server.

use std::time::Duration;

use sysinfo::{Pid, ProcessesToUpdate, System};

/// How often the parent is looked up.
const CHECK_EVERY: Duration = Duration::from_secs(5);
/// Consecutive checks the parent must be gone before the server exits (15 s).
const MISS_LIMIT: u32 = 3;

/// Counts consecutive checks that found the parent gone.
struct Misses {
    count: u32,
    limit: u32,
}

impl Misses {
    fn new(limit: u32) -> Self {
        Self { count: 0, limit }
    }

    /// Records one check; true once the parent has been gone for `limit` checks in a row.
    fn observe(&mut self, parent_alive: bool) -> bool {
        if parent_alive {
            self.count = 0;
        } else {
            self.count = self.count.saturating_add(1);
        }
        self.count >= self.limit
    }
}

/// The parent's PID and start time. The start time tells the parent from a later process that
/// was given the same PID.
struct Parent {
    pid: Pid,
    start_time: u64,
}

impl Parent {
    /// The parent of this process, or None when it cannot be determined.
    fn of_self(sys: &mut System) -> Option<Self> {
        let me = sysinfo::get_current_pid().ok()?;
        sys.refresh_processes(ProcessesToUpdate::Some(&[me]), true);
        let pid = sys.process(me)?.parent()?;
        let start_time = Self::start_time(sys, pid)?;
        Some(Self { pid, start_time })
    }

    fn start_time(sys: &mut System, pid: Pid) -> Option<u64> {
        sys.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
        sys.process(pid).map(|p| p.start_time())
    }

    fn alive(&self, sys: &mut System) -> bool {
        Self::start_time(sys, self.pid) == Some(self.start_time)
    }
}

/// Starts the watch thread. Does nothing when `FIGCRAFT_MCP_NO_PARENT_WATCH=1` or when the parent
/// cannot be determined.
pub fn start() {
    if std::env::var("FIGCRAFT_MCP_NO_PARENT_WATCH").is_ok_and(|v| v.trim() == "1") {
        return;
    }
    let mut sys = System::new();
    let Some(parent) = Parent::of_self(&mut sys) else { return };
    // A failed spawn only loses the extra guard; the stdin check still applies.
    let _ = std::thread::Builder::new().name("parent-watch".into()).spawn(move || {
        let mut misses = Misses::new(MISS_LIMIT);
        loop {
            std::thread::sleep(CHECK_EVERY);
            if misses.observe(parent.alive(&mut sys)) {
                eprintln!("figcraft MCP server: parent process {} is gone, exiting", parent.pid);
                std::process::exit(0);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reaches_the_limit_after_consecutive_misses() {
        let mut m = Misses::new(3);
        assert!(!m.observe(false));
        assert!(!m.observe(false));
        assert!(m.observe(false));
    }

    #[test]
    fn a_live_check_resets_the_count() {
        let mut m = Misses::new(3);
        assert!(!m.observe(false));
        assert!(!m.observe(false));
        assert!(!m.observe(true));
        assert!(!m.observe(false));
        assert!(!m.observe(false));
        assert!(m.observe(false));
    }

    #[test]
    fn a_live_parent_never_reaches_the_limit() {
        let mut m = Misses::new(3);
        for _ in 0..100 {
            assert!(!m.observe(true));
        }
    }

    #[test]
    fn this_process_is_found_with_a_start_time() {
        let mut sys = System::new();
        let me = sysinfo::get_current_pid().unwrap();
        assert!(Parent::start_time(&mut sys, me).is_some());
    }
}
