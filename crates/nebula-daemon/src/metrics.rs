//! Memory readings for the TUI's metrics modal: one machine-wide `ps` sweep,
//! then a per-session sum over each PTY child's process subtree (an agent CLI
//! fans out into node workers, shells, MCP servers — the user cares about the
//! whole tree, not just the root).

use nebula_core::{MetricsSnapshot, SessionMetrics, SessionRef};
use std::collections::{HashMap, HashSet};

/// Take one reading. Blocking (shells out to `ps`); call via spawn_blocking.
pub fn collect(sessions: Vec<(SessionRef, u32)>) -> MetricsSnapshot {
    let table = std::process::Command::new("ps")
        .args(["-axo", "pid=,ppid=,rss="])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .unwrap_or_default();
    snapshot_from_table(
        &table,
        std::process::id(),
        &sessions,
        nebula_core::mem::system_total_bytes().unwrap_or(0),
    )
}

/// Pure core, unit-testable: `table` is `ps -axo pid=,ppid=,rss=` output
/// (one process per line, rss in KB).
fn snapshot_from_table(
    table: &str,
    daemon_pid: u32,
    sessions: &[(SessionRef, u32)],
    system_total_bytes: u64,
) -> MetricsSnapshot {
    let mut rss: HashMap<u32, u64> = HashMap::new();
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for line in table.lines() {
        let mut cols = line.split_whitespace();
        let (Some(pid), Some(ppid), Some(kb)) = (
            cols.next().and_then(|s| s.parse::<u32>().ok()),
            cols.next().and_then(|s| s.parse::<u32>().ok()),
            cols.next().and_then(|s| s.parse::<u64>().ok()),
        ) else {
            continue;
        };
        rss.insert(pid, kb * 1024);
        children.entry(ppid).or_default().push(pid);
    }

    let sessions = sessions
        .iter()
        .map(|(sref, pid)| {
            let (rss_bytes, procs) = subtree_rss(*pid, &rss, &children);
            SessionMetrics { session: sref.clone(), pid: *pid, rss_bytes, procs }
        })
        .collect();

    MetricsSnapshot {
        daemon_pid,
        daemon_rss_bytes: rss.get(&daemon_pid).copied().unwrap_or(0),
        system_total_bytes,
        sessions,
    }
}

/// Sum RSS over `root` and every descendant. The visited set guards against
/// pid-reuse cycles in a racy `ps` snapshot; a root that already exited
/// (absent from the table) reads as zero.
fn subtree_rss(
    root: u32,
    rss: &HashMap<u32, u64>,
    children: &HashMap<u32, Vec<u32>>,
) -> (u64, u32) {
    let mut stack = vec![root];
    let mut seen = HashSet::new();
    let (mut bytes, mut procs) = (0u64, 0u32);
    while let Some(pid) = stack.pop() {
        if !seen.insert(pid) {
            continue;
        }
        if let Some(b) = rss.get(&pid) {
            bytes += b;
            procs += 1;
        }
        if let Some(kids) = children.get(&pid) {
            stack.extend(kids);
        }
    }
    (bytes, procs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nebula_core::{AgentId, TerminalId};

    fn sref(id: &str) -> SessionRef {
        SessionRef::Agent(AgentId(id.into()))
    }

    #[test]
    fn sums_each_sessions_whole_subtree() {
        // daemon 10 → session roots 20 and 30; 20 → {21, 22}, 22 → 23.
        // 99 is an unrelated process and must count nowhere.
        let table = "\
 10     1  1000
 20    10  2000
 21    20   300
 22    20   500
 23    22   200
 30    10    50
 99     1  9999
";
        let snap = snapshot_from_table(
            table,
            10,
            &[(sref("a"), 20), (SessionRef::Terminal(TerminalId("t".into())), 30)],
            0,
        );
        assert_eq!(snap.daemon_rss_bytes, 1000 * 1024);
        assert_eq!(snap.sessions[0].rss_bytes, (2000 + 300 + 500 + 200) * 1024);
        assert_eq!(snap.sessions[0].procs, 4);
        assert_eq!(snap.sessions[1].rss_bytes, 50 * 1024);
        assert_eq!(snap.sessions[1].procs, 1);
    }

    #[test]
    fn exited_session_reads_zero() {
        let snap = snapshot_from_table(" 10 1 100\n", 10, &[(sref("a"), 555)], 0);
        assert_eq!(snap.sessions[0].rss_bytes, 0);
        assert_eq!(snap.sessions[0].procs, 0);
    }

    #[test]
    fn garbage_lines_are_skipped() {
        let table = "not numbers at all\n 20 10 100\n";
        let snap = snapshot_from_table(table, 10, &[(sref("a"), 20)], 0);
        assert_eq!(snap.sessions[0].rss_bytes, 100 * 1024);
        assert_eq!(snap.daemon_rss_bytes, 0);
    }
}
