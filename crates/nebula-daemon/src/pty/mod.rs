pub mod cloud;
pub mod cursor;
pub mod kitty;
mod osc;
pub mod progress;
pub mod question;
pub mod ring;
pub mod title;

use anyhow::{Context, Result};
use cloud::CloudScanner;
use cursor::CursorTracker;
use nebula_core::SessionRef;
use parking_lot::Mutex;
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use progress::ProgressScanner;
use question::QuestionScanner;
use ring::ScrollbackRing;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};

const RING_CAPACITY: usize = 1024 * 1024;
/// Flush coalesced output at this size…
const COALESCE_BYTES: usize = 8 * 1024;
/// …or this long after the previous flush, whichever comes first. A hard
/// deadline (not a quiet-gap timer): a child streaming continuously in small
/// chunks must still flush on time, or output arrives in laggy 8KB lumps.
/// Counted from the last flush rather than from the first pending byte, so
/// output that breaks a silence — the echo of a typed character, a prompt
/// redrawn after Enter — is not held at all (`flush_deadline`).
const COALESCE_HOLD: std::time::Duration = std::time::Duration::from_millis(5);
/// Reader thread → pump channel bound; blocking_send gives natural
/// backpressure against a fire-hosing child.
const READER_CHANNEL_BOUND: usize = 64;
/// After the polite SIGHUP, how long the child gets to exit before its whole
/// process group is SIGKILLed.
const KILL_GRACE: std::time::Duration = std::time::Duration::from_secs(3);
/// Size a session is spawned at when no client is attached to say better
/// (prewarms, respawns after a move, restarts). The first attach resizes it
/// to the real pane, so these only shape the child's first paint.
pub const DEFAULT_COLS: u16 = 80;
pub const DEFAULT_ROWS: u16 = 24;
/// The two bytes every output scanner in this module keys on: ESC opens a
/// CSI/OSC sequence, BEL is the classic OSC terminator.
pub(crate) const ESC: u8 = 0x1b;
pub(crate) const BEL: u8 = 0x07;

/// A `PtySize` in cells only. Nothing here knows pixel dimensions, and
/// leaving them zero is what every caller wants.
fn pty_size(cols: u16, rows: u16) -> PtySize {
    PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    }
}

/// The live process table, one process per line, as `ps -axo
/// pid=,ppid=,pgid=,stat=` prints it; None when `ps` itself fails.
fn ps_table() -> Option<String> {
    let out = std::process::Command::new("ps")
        .args(["-axo", "pid=,ppid=,pgid=,stat="])
        .output()
        .ok()?;
    String::from_utf8(out.stdout).ok()
}

/// One process as the `ps` sweep reports it.
struct ProcRow {
    pid: u32,
    ppid: u32,
    pgid: u32,
    /// The `s` flag of `stat`: the process leads a session of its own, so
    /// something called `setsid` to cut it loose from its terminal.
    session_leader: bool,
}

/// Parse [`ps_table`] output. A row without a `stat` column parses too (as
/// no session leader), so a bare `pid ppid pgid` table works.
fn parse_ps_table(table: &str) -> Vec<ProcRow> {
    table
        .lines()
        .filter_map(|line| {
            let mut cols = line.split_whitespace();
            let pid = cols.next()?.parse().ok()?;
            let ppid = cols.next()?.parse().ok()?;
            let pgid = cols.next()?.parse().ok()?;
            let session_leader = cols.next().is_some_and(|stat| stat.contains('s'));
            Some(ProcRow {
                pid,
                ppid,
                pgid,
                session_leader,
            })
        })
        .collect()
}

/// `root`'s subtree — children, grandchildren, and on down — without
/// `root` itself.
fn descendants(rows: &[ProcRow], root: u32) -> Vec<&ProcRow> {
    let mut children: HashMap<u32, Vec<&ProcRow>> = HashMap::new();
    for row in rows {
        children.entry(row.ppid).or_default().push(row);
    }
    let mut found = Vec::new();
    let mut stack = vec![root];
    let mut seen = HashSet::new();
    while let Some(pid) = stack.pop() {
        if !seen.insert(pid) {
            continue;
        }
        if let Some(kids) = children.remove(&pid) {
            stack.extend(kids.iter().map(|kid| kid.pid));
            found.extend(kids);
        }
    }
    found
}

/// Every process group with a member in `root`'s subtree, `root`'s own
/// first (it leads its PTY session, so that group is its pid, and it is
/// named even when the `ps` sweep fails). An interactive shell running the
/// agent as a job puts it in a group of its own; SIGKILLing the leader's
/// group alone would miss it. Taken while the tree is intact — see `kill`.
fn process_groups_under(root: u32) -> Vec<u32> {
    process_groups_in_table(&ps_table().unwrap_or_default(), root)
}

/// Pure core of [`process_groups_under`] over a [`ps_table`].
fn process_groups_in_table(table: &str, root: u32) -> Vec<u32> {
    let rows = parse_ps_table(table);
    let mut groups = vec![root];
    let own = rows.iter().filter(|row| row.pid == root);
    for row in own.chain(descendants(&rows, root)) {
        if !groups.contains(&row.pgid) {
            groups.push(row.pgid);
        }
    }
    groups
}

/// Is a job the agent cut loose from its terminal still running under
/// `root` — a descendant leading a session of its own? That is how Claude
/// Code runs a backgrounded Bash call or a Monitor watch and how Codex runs
/// a shell command: work that outlives the turn which started it, and that
/// the hook-fed status machine therefore no longer sees. The helpers an
/// agent keeps inside its own session — MCP servers, `caffeinate`, Codex's
/// code-mode host — are not counted, so an idle agent still reads as idle.
/// A failed `ps` counts as busy: never kill what can't be inspected.
pub(crate) fn detached_job_under(root: u32) -> bool {
    match ps_table() {
        Some(table) => detached_job_in_table(&table, root),
        None => true,
    }
}

/// Pure core of [`detached_job_under`] over a [`ps_table`].
fn detached_job_in_table(table: &str, root: u32) -> bool {
    let rows = parse_ps_table(table);
    descendants(&rows, root)
        .iter()
        .any(|row| row.session_leader)
}

/// Every pid in the subtrees rooted at `roots`, the roots included — what
/// the sessions they lead would take down with them. None when `ps` fails.
pub(crate) fn subtree_pids(roots: &[u32]) -> Option<HashSet<u32>> {
    Some(subtree_pids_in_table(&ps_table()?, roots))
}

/// The direct children of `pid`. None when `ps` fails.
pub(crate) fn children_of(pid: u32) -> Option<Vec<u32>> {
    Some(children_in_table(&ps_table()?, pid))
}

/// Pure core of [`children_of`] over a [`ps_table`].
fn children_in_table(table: &str, pid: u32) -> Vec<u32> {
    parse_ps_table(table)
        .iter()
        .filter(|row| row.ppid == pid)
        .map(|row| row.pid)
        .collect()
}

/// Pure core of [`subtree_pids`] over a [`ps_table`].
fn subtree_pids_in_table(table: &str, roots: &[u32]) -> HashSet<u32> {
    let rows = parse_ps_table(table);
    let mut pids: HashSet<u32> = roots.iter().copied().collect();
    for &root in roots {
        pids.extend(descendants(&rows, root).iter().map(|row| row.pid));
    }
    pids
}

/// Broadcast to attached clients (and, later, the status machine).
#[derive(Clone, Debug)]
pub enum PtyEvent {
    Output {
        seq: u64,
        data: Vec<u8>,
    },
    Exited {
        exit_code: Option<i32>,
    },
    /// The child pushed/popped kitty keyboard flags; clients re-encode keys.
    KittyFlags {
        flags: u8,
    },
    /// The child's OSC 9;4 progress state flipped. For agent CLIs this is a
    /// busy/idle edge the status machine trusts — notably it is the *only*
    /// end-of-turn signal after the user cancels, which fires no hook.
    Progress {
        busy: bool,
    },
    /// The child set its window title (OSC 0/2). Claude Code's carries the
    /// session's name (`✳ Fix Login Redirect`), and `/rename` — which
    /// fires no hook — shows up here first (see `pty::title`).
    Title {
        title: String,
    },
    /// The child printed the title Claude Cloud gave the session it
    /// created — the line before the id. Only scanned for on `--cloud`
    /// launches (`arm_cloud_scan`).
    CloudTitle {
        title: String,
    },
    /// The child printed the id of the Claude Cloud session it created.
    /// Only scanned for on `--cloud` launches (`arm_cloud_scan`).
    CloudSession {
        id: String,
    },
    /// Cursor's ask-question dialog came up or went away. Only scanned for
    /// on cursor-agent sessions (`arm_question_scan`), which fire no hook
    /// for it (see `pty::question`).
    Question {
        open: bool,
    },
}

enum ReaderMsg {
    Data(Vec<u8>),
    Eof { exit_code: Option<i32> },
}

pub struct PtySession {
    pub sref: SessionRef,
    // PTY lock order: writer/master are independent operation locks; scanner
    // locks are fed one at a time before appending to `ring`. The only
    // intentional nested scanner path is cursor/question setup before a ring
    // snapshot so no bytes are missed; code must not take `ring` and then a
    // scanner lock.
    writer: Mutex<Box<dyn Write + Send>>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    /// Child pid: drives the SIGHUP → SIGKILL escalation (the child is its
    /// PTY session's leader — portable-pty does setsid — so pgid == pid,
    /// though an agent run as a job under a login shell sits in a group of
    /// its own below it) and the metrics modal's process-tree sums.
    pub child_pid: Option<u32>,
    pub ring: Mutex<ScrollbackRing>,
    pub events: broadcast::Sender<PtyEvent>,
    /// Last applied size, for the attach-time SIGWINCH jiggle.
    last_size: Mutex<(u16, u16)>,
    /// Kitty keyboard negotiation state, fed by the pump from live output.
    kitty: Mutex<kitty::KittyScanner>,
    /// OSC 9;4 busy/idle tracking, likewise fed from live output.
    progress: Mutex<ProgressScanner>,
    /// OSC 0/2 window-title tracking, likewise fed from live output.
    title: Mutex<title::TitleScanner>,
    /// Claude Cloud session id scanner; `None` until a `--cloud` launch
    /// arms it, so ordinary sessions pay nothing.
    cloud: Mutex<Option<CloudScanner>>,
    /// Cursor's ask-question dialog watcher; `None` until a cursor-agent
    /// session arms it, so other sessions keep no screen for it.
    question: Mutex<Option<QuestionScanner>>,
    /// Screen for answering cursor position reports; `None` until the child
    /// first asks, so sessions that never do parse nothing.
    cursor: Mutex<Option<CursorTracker>>,
}

pub struct SpawnSpec {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: std::path::PathBuf,
    /// Extra env vars (NEBULA_* for agents). Plain terminals get none.
    pub env: Vec<(String, String)>,
    /// Env var names to scrub from the inherited environment. Only ever a
    /// fixed list (the agent-session vars), so it is borrowed, not built.
    pub scrub_env: &'static [&'static str],
    pub cols: u16,
    pub rows: u16,
}

impl PtySession {
    /// Spawn the child in a fresh PTY and start its reader thread + pump task.
    pub fn spawn(sref: SessionRef, spec: SpawnSpec) -> Result<Arc<Self>> {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(pty_size(spec.cols, spec.rows))
            .context("openpty")?;

        let mut cmd = CommandBuilder::new(&spec.program);
        cmd.args(&spec.args);
        cmd.cwd(&spec.cwd);
        // The child paints nebula's grid, not the terminal the daemon was
        // started from: name that grid and drop the colour overrides the
        // daemon inherited. Agent launches restate all three after the
        // login shell's profile too (`login_shell_wrap`).
        cmd.env("TERM", nebula_core::env::PANE_TERM);
        cmd.env("COLORTERM", nebula_core::env::PANE_COLORTERM);
        for name in nebula_core::env::PANE_COLOR_OVERRIDES {
            cmd.env_remove(name);
        }
        for name in spec.scrub_env {
            cmd.env_remove(name);
        }
        for (k, v) in &spec.env {
            cmd.env(k, v);
        }

        let mut child = pair
            .slave
            .spawn_command(cmd)
            .context("spawn child in pty")?;
        drop(pair.slave);

        let child_pid = child.process_id();
        let reader = pair.master.try_clone_reader().context("clone pty reader")?;
        let writer = pair.master.take_writer().context("take pty writer")?;
        let reap: Reap = Box::new(move || child.wait().ok().map(|st| st.exit_code() as i32));

        Ok(Self::start(
            Self {
                sref,
                writer: Mutex::new(writer),
                master: Mutex::new(pair.master),
                child_pid,
                ring: Mutex::new(ScrollbackRing::new(RING_CAPACITY)),
                events: broadcast::channel(256).0,
                last_size: Mutex::new((spec.cols, spec.rows)),
                kitty: Mutex::new(kitty::KittyScanner::new()),
                progress: Mutex::new(ProgressScanner::new()),
                title: Mutex::new(title::TitleScanner::new()),
                cloud: Mutex::new(None),
                question: Mutex::new(None),
                cursor: Mutex::new(None),
            },
            reader,
            reap,
        ))
    }

    /// Take over a session an earlier image of this daemon ran, from what
    /// [`Self::carry`] wrote down: the master fd survived the exec, and the
    /// child is still this process's child, so it is reaped as before.
    pub fn adopt(carried: Carried) -> Result<Arc<Self>> {
        // SAFETY: the fd was named by the image that exec'd us, which held it
        // open across the exec for exactly this; nothing else here owns it.
        let fd = unsafe { OwnedFd::from_raw_fd(carried.master_fd) };
        set_cloexec(fd.as_raw_fd(), true)?;
        let master = AdoptedMaster { fd };
        let reader = master.try_clone_reader().context("clone pty reader")?;
        let writer = master.take_writer().context("take pty writer")?;
        let pid = carried.pid;
        let reap: Reap = Box::new(move || reap_pid(pid));
        let (cols, rows) = carried.size;
        let cloud_scan = carried.cloud_scan;
        let question_scan = carried.question_scan;
        let session = Self::start(
            Self {
                sref: carried.sref,
                writer: Mutex::new(writer),
                master: Mutex::new(Box::new(master)),
                child_pid: Some(pid),
                ring: Mutex::new(ScrollbackRing::restored(
                    RING_CAPACITY,
                    carried.ring_start_seq,
                    &carried.ring,
                )),
                events: broadcast::channel(256).0,
                last_size: Mutex::new((cols, rows)),
                kitty: Mutex::new(kitty::KittyScanner::restored(
                    carried.kitty_stack,
                    carried.bracketed_paste,
                )),
                progress: Mutex::new(ProgressScanner::restored(carried.progress_busy)),
                title: Mutex::new(title::TitleScanner::restored(carried.title)),
                cloud: Mutex::new(None),
                question: Mutex::new(None),
                cursor: Mutex::new(None),
            },
            reader,
            reap,
        );
        // Armed before anything subscribes: what the ring already shows,
        // the previous image already acted on.
        if cloud_scan {
            session.arm_cloud_scan();
        }
        if question_scan {
            session.arm_question_scan();
        }
        Ok(session)
    }

    fn start(session: Self, reader: Box<dyn Read + Send>, reap: Reap) -> Arc<Self> {
        let session = Arc::new(session);
        let (tx, rx) = mpsc::channel::<ReaderMsg>(READER_CHANNEL_BOUND);
        spawn_reader_thread(reader, reap, tx);
        tokio::spawn(pump(session.clone(), rx));
        session
    }

    /// Everything a new image of the daemon needs to take this session
    /// over (see [`Self::adopt`]), with the master fd left open across the
    /// exec. None for a session with no child pid to reap. Output the
    /// reader has read but the pump has not yet added to the ring is lost;
    /// the attach that follows repaints the screen anyway.
    pub fn carry(&self) -> Result<Option<Carried>> {
        let (Some(pid), Some(master_fd)) = (self.child_pid, self.master.lock().as_raw_fd()) else {
            return Ok(None);
        };
        set_cloexec(master_fd, false)?;
        let (ring_start_seq, ring) = self.ring.lock().snapshot_from(None);
        let kitty = self.kitty.lock();
        Ok(Some(Carried {
            sref: self.sref.clone(),
            master_fd,
            pid,
            size: *self.last_size.lock(),
            ring_start_seq,
            ring,
            kitty_stack: kitty.stack().to_vec(),
            bracketed_paste: kitty.bracketed_paste(),
            progress_busy: self.progress.lock().busy(),
            title: self.title.lock().title().map(str::to_string),
            cloud_scan: self.cloud.lock().is_some(),
            question_scan: self.question.lock().is_some(),
        }))
    }

    /// Undo [`Self::carry`]'s hold on the master fd: the exec it was for
    /// failed, and this image keeps running.
    pub fn uncarry(&self) {
        if let Some(fd) = self.master.lock().as_raw_fd() {
            let _ = set_cloexec(fd, true);
        }
    }

    pub fn write_input(&self, data: &[u8]) -> Result<()> {
        let mut w = self.writer.lock();
        w.write_all(data)?;
        w.flush()?;
        Ok(())
    }

    pub fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        let master = self.master.lock();
        master.resize(pty_size(cols, rows))?;
        *self.last_size.lock() = (cols, rows);
        if let Some(cursor) = self.cursor.lock().as_mut() {
            cursor.resize(cols, rows);
        }
        if let Some(question) = self.question.lock().as_mut() {
            question.resize(cols, rows);
        }
        Ok(())
    }

    /// Resize on attach. The kernel only delivers SIGWINCH on a *change*, so
    /// when the requested size equals the current one, jiggle (rows-1 then
    /// back) to force a full-screen repaint — the dtach trick.
    pub fn resize_with_jiggle(&self, cols: u16, rows: u16) -> Result<()> {
        let same = { *self.last_size.lock() == (cols, rows) };
        if same && rows > 1 {
            let master = self.master.lock();
            master.resize(pty_size(cols, rows - 1))?;
            master.resize(pty_size(cols, rows))?;
            Ok(())
        } else {
            self.resize(cols, rows)
        }
    }

    /// SIGHUP the child, then SIGKILL every process group under it if it
    /// hasn't exited within [`KILL_GRACE`]. The sweep also reaps grandchildren
    /// that would otherwise hold the slave fd open (no EOF → reader thread,
    /// pump task, and the 1MB ring all pinned forever) — including an agent
    /// the login shell forked into a job group of its own, which a kill of
    /// the leader's group alone would leave running with its pane gone.
    pub fn kill(&self) {
        // Subscribe before signalling so an immediate exit can't be missed.
        let mut rx = self.events.subscribe();
        // Sweep before the polite signal: a shell that dies of it leaves the
        // job it was running to init, where no walk from `pid` would find
        // it afterwards. ~10ms, and a kill is rare.
        let mut groups = self.child_pid.map(process_groups_under).unwrap_or_default();
        let Some(pid) = self.child_pid else { return };
        let _ = nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(pid as i32),
            nix::sys::signal::Signal::SIGHUP,
        );
        let sref = self.sref.clone();
        // Watchdog on a plain thread: it must not hold the session Arc (that
        // would pin the ring), and it outlives any tokio context `kill` was
        // called from.
        std::thread::Builder::new()
            .name("pty-kill-watchdog".into())
            .stack_size(256 * 1024)
            .spawn(move || {
                use nix::sys::signal::{killpg, Signal};
                use nix::unistd::Pid;
                let deadline = std::time::Instant::now() + KILL_GRACE;
                let nix_pid = Pid::from_raw(pid as i32);
                let mut child_gone = false;
                while std::time::Instant::now() < deadline {
                    loop {
                        use tokio::sync::broadcast::error::TryRecvError;
                        match rx.try_recv() {
                            Ok(PtyEvent::Exited { .. }) | Err(TryRecvError::Closed) => {
                                child_gone = true;
                                break;
                            }
                            Ok(_) | Err(TryRecvError::Lagged(_)) => continue,
                            Err(TryRecvError::Empty) => break,
                        }
                    }
                    // Reaped (ESRCH) strictly precedes the Exited broadcast,
                    // so this also covers an Exited lost to channel lag.
                    if child_gone || nix::sys::signal::kill(nix_pid, None).is_err() {
                        child_gone = true;
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                if child_gone {
                    // A shell that exited on its own hung up its jobs first;
                    // one the signal took outright left them to init (and on
                    // macOS the leader's death revokes the slave, so the
                    // session reads as exited with the job still running).
                    // Any group of the sweep still standing is the latter:
                    // it gets the hangup the shell owed it, and its own
                    // grace period.
                    let orphans: Vec<Pid> = groups
                        .iter()
                        .filter(|&&g| g != pid)
                        .map(|&g| Pid::from_raw(g as i32))
                        .filter(|&g| killpg(g, None).is_ok())
                        .collect();
                    if orphans.is_empty() {
                        return;
                    }
                    for g in &orphans {
                        let _ = killpg(*g, Signal::SIGHUP);
                    }
                    let deadline = std::time::Instant::now() + KILL_GRACE;
                    while std::time::Instant::now() < deadline
                        && orphans.iter().any(|&g| killpg(g, None).is_ok())
                    {
                        std::thread::sleep(std::time::Duration::from_millis(100));
                    }
                    if orphans.iter().all(|&g| killpg(g, None).is_err()) {
                        return;
                    }
                }
                // Whatever appeared since the first sweep, then the lot.
                for pgid in process_groups_under(pid) {
                    if !groups.contains(&pgid) {
                        groups.push(pgid);
                    }
                }
                tracing::warn!(session = ?sref, pid, ?groups, "still running after SIGHUP — SIGKILLing its process groups");
                for pgid in groups {
                    let _ = killpg(Pid::from_raw(pgid as i32), Signal::SIGKILL);
                }
            })
            .expect("spawn pty kill watchdog");
    }

    /// Ring snapshot for attach replay: (base_seq, bytes).
    ///
    /// A replay that rebuilds the client's screen from a ring that has
    /// wrapped opens with the modes whose set may have fallen off its head
    /// — bracketed paste, which claude turns on once at startup and the
    /// client reads to paste the way a terminal would (#107) — and
    /// `base_seq` backs up by their length, so the client's byte count
    /// still ends where the ring does.
    pub fn snapshot(&self, from_seq: Option<u64>) -> (u64, Vec<u8>) {
        let (base_seq, data) = self.ring.lock().snapshot_from(from_seq);
        let modes: &[u8] = if self.kitty.lock().bracketed_paste() {
            b"\x1b[?2004h"
        } else {
            b""
        };
        with_modes(from_seq, base_seq, data, modes)
    }

    /// The end of the ring for a grid card's preview (`TailOutput`): the
    /// last `max_bytes`, with the PTY's size for the client to lay them
    /// out against. No bytes when the ring has not grown past `after_seq`
    /// — the card already has them.
    pub fn tail(&self, max_bytes: usize, after_seq: Option<u64>) -> nebula_core::OutputTail {
        let (cols, rows) = *self.last_size.lock();
        let ring = self.ring.lock();
        let end_seq = ring.end_seq();
        let data = if after_seq == Some(end_seq) {
            Vec::new()
        } else {
            ring.tail(max_bytes)
        };
        nebula_core::OutputTail {
            cols,
            rows,
            end_seq,
            data,
        }
    }

    /// The child's current kitty keyboard flags (0 = legacy).
    pub fn kitty_flags(&self) -> u8 {
        self.kitty.lock().flags()
    }

    /// The child's advertised OSC 9;4 busy state, or `None` if it never
    /// advertised one (a CLI without a progress bar, or one not started yet).
    pub fn progress_busy(&self) -> Option<bool> {
        self.progress.lock().busy()
    }

    /// The child's current window title, or `None` if it never set one.
    /// Test-only: the daemon reads titles off the scanner's change edge.
    #[cfg(test)]
    pub fn window_title(&self) -> Option<String> {
        self.title.lock().title().map(str::to_string)
    }

    /// Start watching this child's output for the Claude Cloud session id
    /// it prints on creation (see `pty::cloud`). Output that already landed
    /// in the ring is scanned first, so arming a moment after spawn cannot
    /// miss a fast-printing child; the sighting then arrives as
    /// `PtyEvent::CloudSession`.
    pub fn arm_cloud_scan(&self) {
        let mut scanner = CloudScanner::new();
        let (_, replay) = self.snapshot(None);
        let sightings = scanner.feed(&replay);
        *self.cloud.lock() = Some(scanner);
        for sighting in sightings {
            let _ = self.events.send(sighting.into());
        }
    }

    /// Start watching this cursor-agent child's screen for its
    /// ask-question dialog (see `pty::question`); edges then arrive as
    /// `PtyEvent::Question`. The screen is built from the ring under the
    /// scanner's lock, which the pump takes before it appends a chunk, so
    /// no byte is missed or fed twice.
    pub fn arm_question_scan(&self) {
        let mut slot = self.question.lock();
        let (cols, rows) = *self.last_size.lock();
        let (_, history) = self.snapshot(None);
        *slot = Some(QuestionScanner::new(cols, rows, &history));
    }

    /// The bytes owed to the child for one chunk's queries, in query order.
    /// A cursor report reads the screen as it stood just past its query, so
    /// the tracker takes `chunk` up to each one in turn, then the rest: once
    /// built — from the ring, which does not hold `chunk` yet, at the PTY's
    /// size — it follows every byte.
    fn query_replies(&self, replies: Vec<kitty::Reply>, chunk: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut cursor = self.cursor.lock();
        let mut fed = 0;
        for reply in replies {
            match reply {
                kitty::Reply::Bytes(bytes) => out.extend_from_slice(&bytes),
                kitty::Reply::CursorPosition { at } => {
                    let tracker = cursor.get_or_insert_with(|| {
                        let (cols, rows) = *self.last_size.lock();
                        let (_, history) = self.snapshot(None);
                        CursorTracker::new(cols, rows, &history)
                    });
                    tracker.feed(&chunk[fed..at]);
                    fed = at;
                    out.extend_from_slice(&tracker.report());
                }
            }
        }
        if let Some(tracker) = cursor.as_mut() {
            tracker.feed(&chunk[fed..]);
        }
        out
    }
}

/// What an IN-PLACE RESTART carries of one live session across the exec.
#[derive(Debug, Serialize, Deserialize)]
pub struct Carried {
    pub sref: SessionRef,
    /// The PTY master, open in the new image under the same number.
    pub master_fd: RawFd,
    pub pid: u32,
    pub size: (u16, u16),
    pub ring_start_seq: u64,
    #[serde(with = "serde_bytes")]
    pub ring: Vec<u8>,
    #[serde(default)]
    pub kitty_stack: Vec<u8>,
    #[serde(default)]
    pub bracketed_paste: bool,
    #[serde(default)]
    pub progress_busy: Option<bool>,
    #[serde(default)]
    pub title: Option<String>,
    /// A `claude --cloud` create still watched for the session id it
    /// prints (`arm_cloud_scan`).
    #[serde(default)]
    pub cloud_scan: bool,
    /// A cursor-agent session watched for its ask-question dialog
    /// (`arm_question_scan`).
    #[serde(default)]
    pub question_scan: bool,
}

/// Blocks until the child exits; its exit code, when the OS gave one.
type Reap = Box<dyn FnOnce() -> Option<i32> + Send>;

/// `waitpid` for a child this image did not spawn, but inherited across an
/// exec. A signal death reads as 1, the code portable-pty gives it.
fn reap_pid(pid: u32) -> Option<i32> {
    use nix::sys::wait::{waitpid, WaitStatus};
    let pid = nix::unistd::Pid::from_raw(pid as i32);
    loop {
        match waitpid(pid, None) {
            Ok(WaitStatus::Exited(_, code)) => return Some(code),
            Ok(WaitStatus::Signaled(..)) => return Some(1),
            Ok(_) | Err(nix::errno::Errno::EINTR) => continue,
            Err(_) => return None,
        }
    }
}

pub(crate) fn set_cloexec(fd: RawFd, on: bool) -> Result<()> {
    use nix::fcntl::{fcntl, FcntlArg, FdFlag};
    // SAFETY: only ever called on an fd this process holds open.
    let borrowed = unsafe { std::os::fd::BorrowedFd::borrow_raw(fd) };
    let mut flags = FdFlag::from_bits_truncate(fcntl(borrowed, FcntlArg::F_GETFD)?);
    flags.set(FdFlag::FD_CLOEXEC, on);
    fcntl(borrowed, FcntlArg::F_SETFD(flags))?;
    Ok(())
}

/// A PTY master inherited across an exec, where portable-pty has no way to
/// wrap an fd it did not open itself.
struct AdoptedMaster {
    fd: OwnedFd,
}

impl AdoptedMaster {
    fn dup(&self) -> anyhow::Result<std::fs::File> {
        Ok(std::fs::File::from(self.fd.try_clone()?))
    }
}

impl MasterPty for AdoptedMaster {
    fn resize(&self, size: PtySize) -> anyhow::Result<()> {
        let ws = nix::libc::winsize {
            ws_row: size.rows,
            ws_col: size.cols,
            ws_xpixel: size.pixel_width,
            ws_ypixel: size.pixel_height,
        };
        // SAFETY: TIOCSWINSZ reads one winsize from the pointer, which
        // outlives the call.
        if unsafe { nix::libc::ioctl(self.fd.as_raw_fd(), nix::libc::TIOCSWINSZ, &ws) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(())
    }

    fn get_size(&self) -> anyhow::Result<PtySize> {
        let mut ws: nix::libc::winsize = unsafe { std::mem::zeroed() };
        // SAFETY: TIOCGWINSZ writes one winsize through the pointer.
        if unsafe { nix::libc::ioctl(self.fd.as_raw_fd(), nix::libc::TIOCGWINSZ, &mut ws) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(PtySize {
            rows: ws.ws_row,
            cols: ws.ws_col,
            pixel_width: ws.ws_xpixel,
            pixel_height: ws.ws_ypixel,
        })
    }

    fn try_clone_reader(&self) -> anyhow::Result<Box<dyn Read + Send>> {
        Ok(Box::new(self.dup()?))
    }

    fn take_writer(&self) -> anyhow::Result<Box<dyn Write + Send>> {
        Ok(Box::new(self.dup()?))
    }

    fn process_group_leader(&self) -> Option<nix::libc::pid_t> {
        match unsafe { nix::libc::tcgetpgrp(self.fd.as_raw_fd()) } {
            pid if pid > 0 => Some(pid),
            _ => None,
        }
    }

    fn as_raw_fd(&self) -> Option<RawFd> {
        Some(self.fd.as_raw_fd())
    }

    fn tty_name(&self) -> Option<std::path::PathBuf> {
        None
    }
}

/// PTY reads are blocking → dedicated thread per session. After EOF it reaps
/// the child to get the exit code.
fn spawn_reader_thread(mut reader: Box<dyn Read + Send>, reap: Reap, tx: mpsc::Sender<ReaderMsg>) {
    std::thread::Builder::new()
        .name("pty-reader".into())
        .stack_size(256 * 1024)
        .spawn(move || {
            let mut buf = [0u8; 16 * 1024];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if tx
                            .blocking_send(ReaderMsg::Data(buf[..n].to_vec()))
                            .is_err()
                        {
                            break;
                        }
                    }
                }
            }
            let exit_code = reap();
            let _ = tx.blocking_send(ReaderMsg::Eof { exit_code });
        })
        .expect("spawn pty reader thread");
}

/// The ring's replay `data` from `base_seq`, opened with `modes` when it
/// rebuilds a screen that lost the ring's head. The whole history
/// (`base_seq` 0) and a continuation of a screen the client kept (its
/// `from_seq`) lost nothing and go as they are. So does a replay whose
/// backed-up start would be exactly where the client asked to resume:
/// it would take `modes` for the bytes it missed and keep its screen.
fn with_modes(from_seq: Option<u64>, base_seq: u64, data: Vec<u8>, modes: &[u8]) -> (u64, Vec<u8>) {
    let start = base_seq.saturating_sub(modes.len() as u64);
    if modes.is_empty() || start == 0 || from_seq == Some(base_seq) || from_seq == Some(start) {
        return (base_seq, data);
    }
    let mut replay = Vec::with_capacity(modes.len() + data.len());
    replay.extend_from_slice(modes);
    replay.extend_from_slice(&data);
    (start, replay)
}

/// When output that arrived at `now` has to be on its way to the clients.
/// The hold exists to turn a stream of small writes into fewer, larger
/// events — so it is spent only while there is a stream: a flush less than
/// [`COALESCE_HOLD`] ago means more is likely right behind, and the bytes
/// wait out the rest of that window. After a quiet spell they go at once.
/// That is every keystroke's echo: held, it reached the TUI 5 ms late on
/// every character typed into a pane, which the INPUT LATENCY PROBE put at
/// half of what the key took to show. A stream still flushes at most once
/// per hold, exactly as before; the one extra event is at its head.
fn flush_deadline(
    now: tokio::time::Instant,
    last_flush: Option<tokio::time::Instant>,
) -> tokio::time::Instant {
    match last_flush {
        Some(last) if now < last + COALESCE_HOLD => last + COALESCE_HOLD,
        _ => now,
    }
}

/// Drains the reader channel: append to the ring (always — detach is free),
/// coalesce bursts, broadcast to whoever is attached.
async fn pump(session: Arc<PtySession>, mut rx: mpsc::Receiver<ReaderMsg>) {
    let mut pending: Vec<u8> = Vec::new();

    let flush = |session: &PtySession, pending: &mut Vec<u8>| {
        if pending.is_empty() {
            return;
        }
        // Terminal queries (kitty keyboard, DA1, DSR) ride in the output
        // stream; nothing else would ever answer them (tmux does the same).
        let actions = session.kitty.lock().feed(pending);
        let reply = session.query_replies(actions.replies, pending);
        if !reply.is_empty() {
            if let Err(e) = session.write_input(&reply) {
                tracing::warn!(error = %e, "terminal query reply write failed");
            }
        }
        let busy_edge = session.progress.lock().feed(pending);
        let title_change = session.title.lock().feed(pending);
        let cloud_sightings = match session.cloud.lock().as_mut() {
            Some(scanner) => scanner.feed(pending),
            None => Vec::new(),
        };
        let question_edge = session
            .question
            .lock()
            .as_mut()
            .and_then(|scanner| scanner.feed(pending, std::time::Instant::now()));
        let seq = session.ring.lock().append(pending);
        let _ = session.events.send(PtyEvent::Output {
            seq,
            data: std::mem::take(pending),
        });
        if let Some(flags) = actions.flags_changed {
            tracing::debug!(session = ?session.sref, flags, "child kitty flags changed");
            let _ = session.events.send(PtyEvent::KittyFlags { flags });
        }
        if let Some(busy) = busy_edge {
            tracing::debug!(session = ?session.sref, busy, "child progress state changed");
            let _ = session.events.send(PtyEvent::Progress { busy });
        }
        if let Some(title) = title_change {
            tracing::debug!(session = ?session.sref, %title, "child window title changed");
            let _ = session.events.send(PtyEvent::Title { title });
        }
        for sighting in cloud_sightings {
            tracing::info!(session = ?session.sref, ?sighting, "cloud sighting in child output");
            let _ = session.events.send(sighting.into());
        }
        if let Some(open) = question_edge {
            tracing::debug!(session = ?session.sref, open, "child question dialog changed");
            let _ = session.events.send(PtyEvent::Question { open });
        }
    };

    let mut last_flush: Option<tokio::time::Instant> = None;
    'outer: loop {
        if pending.is_empty() {
            match rx.recv().await {
                Some(ReaderMsg::Data(d)) => pending.extend_from_slice(&d),
                Some(ReaderMsg::Eof { exit_code }) => {
                    let _ = session.events.send(PtyEvent::Exited { exit_code });
                    break;
                }
                None => break,
            }
        }
        // Coalesce until the deadline or the size cap; the deadline is fixed
        // when the first pending byte arrives so continuous streams still
        // flush on time. Biased toward the channel: a deadline that has
        // already passed (the quiet-spell case) still takes along whatever
        // the reader has queued, so one write read in two pieces is one event.
        let deadline = flush_deadline(tokio::time::Instant::now(), last_flush);
        while pending.len() < COALESCE_BYTES {
            tokio::select! {
                biased;
                msg = rx.recv() => match msg {
                    Some(ReaderMsg::Data(d)) => pending.extend_from_slice(&d),
                    Some(ReaderMsg::Eof { exit_code }) => {
                        flush(&session, &mut pending);
                        let _ = session.events.send(PtyEvent::Exited { exit_code });
                        break 'outer;
                    }
                    None => {
                        flush(&session, &mut pending);
                        break 'outer;
                    }
                },
                _ = tokio::time::sleep_until(deadline) => break,
            }
        }
        flush(&session, &mut pending);
        last_flush = Some(tokio::time::Instant::now());
    }
    tracing::info!(session = ?session.sref, "pty pump ended");
}

#[cfg(test)]
mod tests {
    use super::*;
    use nebula_core::AgentId;

    /// Output that breaks a silence is not held: a typed character's echo
    /// leaves the DAEMON the moment it is read.
    #[test]
    fn output_after_a_quiet_spell_is_flushed_at_once() {
        let now = tokio::time::Instant::now();
        assert_eq!(flush_deadline(now, None), now, "the session's first bytes");
        let long_ago = now - COALESCE_HOLD * 10;
        assert_eq!(flush_deadline(now, Some(long_ago)), now);
        assert_eq!(flush_deadline(now, Some(now - COALESCE_HOLD)), now);
    }

    /// A stream is still coalesced: bytes arriving inside the hold of the
    /// last flush wait for that hold to end, so the event rate under
    /// sustained output is what it was — one flush per hold at most.
    #[test]
    fn output_inside_the_hold_waits_for_it_to_end() {
        let now = tokio::time::Instant::now();
        let just_flushed = now - std::time::Duration::from_millis(1);
        assert_eq!(
            flush_deadline(now, Some(just_flushed)),
            just_flushed + COALESCE_HOLD
        );
    }

    /// Only a replay that rebuilds a screen from past the ring's head opens
    /// with the modes, and the client's byte count still lands on the end.
    #[test]
    fn modes_open_only_a_replay_that_lost_the_ring_head() {
        let modes = b"\x1b[?2004h";
        let (start, replay) = with_modes(None, 100, b"tail".to_vec(), modes);
        assert_eq!(replay, b"\x1b[?2004htail");
        assert_eq!(start + replay.len() as u64, 104, "ends where the ring does");
        // A resume point that fell off the ring rebuilds the screen too.
        assert_eq!(with_modes(Some(50), 100, b"tail".to_vec(), modes).0, 92);

        let untouched = (100, b"tail".to_vec());
        assert_eq!(with_modes(None, 100, b"tail".to_vec(), b""), untouched);
        // A continuation of a screen the client kept.
        assert_eq!(
            with_modes(Some(100), 100, b"tail".to_vec(), modes),
            untouched
        );
        // A start that would read as that continuation.
        assert_eq!(
            with_modes(Some(92), 100, b"tail".to_vec(), modes),
            untouched
        );
        // The whole history, or too little lost to back up over.
        assert_eq!(
            with_modes(None, 0, b"all".to_vec(), modes),
            (0, b"all".to_vec())
        );
        assert_eq!(
            with_modes(None, 8, b"tail".to_vec(), modes),
            (8, b"tail".to_vec())
        );
    }

    /// A child that turned bracketed paste on and has since printed more
    /// than the ring holds — claude, a long session later — still has it on
    /// a screen rebuilt from the replay, and the replay ends on the ring's
    /// last byte (#107).
    #[tokio::test]
    async fn a_wrapped_replay_keeps_bracketed_paste() {
        let session = PtySession::spawn(
            SessionRef::Agent(AgentId::generate()),
            SpawnSpec {
                program: "/bin/sh".into(),
                args: vec![
                    "-c".into(),
                    format!(
                        "printf '\\033[?2004h'; head -c {} /dev/zero | tr '\\000' x",
                        RING_CAPACITY + 4096
                    ),
                ],
                cwd: std::env::temp_dir(),
                env: vec![],
                scrub_env: &[],
                cols: DEFAULT_COLS,
                rows: DEFAULT_ROWS,
            },
        )
        .unwrap();
        let mut rx = session.events.subscribe();
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                match rx.recv().await {
                    Ok(PtyEvent::Exited { .. }) => break,
                    Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(e) => panic!("event stream ended: {e}"),
                }
            }
        })
        .await
        .expect("child exits within 10s");

        let (head, raw) = session.ring.lock().snapshot_from(None);
        assert!(head > 0, "the ring wrapped");
        let (start, replay) = session.snapshot(None);
        assert_eq!(start + replay.len() as u64, head + raw.len() as u64);
        let mut screen = vt100::Parser::new(DEFAULT_ROWS, DEFAULT_COLS, 0);
        screen.process(&replay);
        assert!(screen.screen().bracketed_paste());
        // A client that kept its screen gets the ring as it is.
        assert_eq!(session.snapshot(Some(head)), (head, raw));
    }

    /// A window title set by the child reaches subscribers as its own
    /// event, and the session remembers it.
    #[tokio::test]
    async fn window_title_is_read_off_the_output() {
        let session = PtySession::spawn(
            SessionRef::Agent(AgentId::generate()),
            SpawnSpec {
                program: "/bin/sh".into(),
                args: vec![
                    "-c".into(),
                    "printf '\\033]0;✳ Fix Login\\007'; sleep 2".into(),
                ],
                cwd: std::env::temp_dir(),
                env: vec![],
                scrub_env: &[],
                cols: DEFAULT_COLS,
                rows: DEFAULT_ROWS,
            },
        )
        .unwrap();
        let mut rx = session.events.subscribe();
        let title = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                match rx.recv().await {
                    Ok(PtyEvent::Title { title }) => break title,
                    Ok(PtyEvent::Exited { .. }) => panic!("child exited before setting a title"),
                    Ok(_) => continue,
                    Err(e) => panic!("event stream ended: {e}"),
                }
            }
        })
        .await
        .expect("title event within 10s");
        assert_eq!(title, "✳ Fix Login");
        assert_eq!(session.window_title().as_deref(), Some("✳ Fix Login"));
        session.kill();
    }

    /// The pane is nebula's grid, not the terminal the daemon was started
    /// from: every child hears `TERM`/`COLORTERM` for that grid, and a
    /// `NO_COLOR` or `FORCE_COLOR` the daemon inherited never reaches it.
    #[tokio::test]
    async fn child_is_told_the_pane_is_a_truecolor_terminal() {
        // The daemon's own environment is the base the child is built from,
        // so the overrides have to sit there for the scrub to be exercised.
        // Only this test sets them, and only until the spawn has read them;
        // whatever was there before is put back.
        let before: Vec<(&str, Option<std::ffi::OsString>)> = ["NO_COLOR", "FORCE_COLOR", "TERM"]
            .into_iter()
            .map(|name| (name, std::env::var_os(name)))
            .collect();
        std::env::set_var("NO_COLOR", "1");
        std::env::set_var("FORCE_COLOR", "0");
        std::env::set_var("TERM", "foot");
        let session = PtySession::spawn(
            SessionRef::Agent(AgentId::generate()),
            SpawnSpec {
                program: "/bin/sh".into(),
                args: vec![
                    "-c".into(),
                    r#"printf 'env=%s|%s|%s|%s' "$TERM" "$COLORTERM" "${NO_COLOR-unset}" "${FORCE_COLOR-unset}""#
                        .into(),
                ],
                cwd: std::env::temp_dir(),
                env: vec![],
                scrub_env: &[],
                cols: DEFAULT_COLS,
                rows: DEFAULT_ROWS,
            },
        )
        .unwrap();
        for (name, value) in before {
            match value {
                Some(value) => std::env::set_var(name, value),
                None => std::env::remove_var(name),
            }
        }
        let mut rx = session.events.subscribe();
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                match rx.recv().await {
                    Ok(PtyEvent::Exited { .. }) => break,
                    Ok(_) => continue,
                    Err(e) => panic!("event stream ended: {e}"),
                }
            }
        })
        .await
        .expect("child exits within 10s");
        let (_, bytes) = session.snapshot(None);
        let out = String::from_utf8_lossy(&bytes);
        assert!(
            out.contains("env=xterm-256color|truecolor|unset|unset"),
            "child saw: {out:?}"
        );
    }

    /// A child asking where its cursor is — crossterm's `cursor::position()`,
    /// which timed out inside nebula before (#66) — is answered from the
    /// screen as it stood at the query, not after the rest of the chunk.
    #[tokio::test]
    async fn cursor_position_query_is_answered_from_the_screen() {
        let session = PtySession::spawn(
            SessionRef::Agent(AgentId::generate()),
            SpawnSpec {
                program: "/bin/sh".into(),
                args: vec![
                    "-c".into(),
                    // `min 6`: the read waits for the whole six-byte reply.
                    "stty -icanon -echo min 6 time 50; \
                     printf 'hi\\033[3;7H\\033[6nbye'; \
                     printf 'REPLY:'; dd bs=6 count=1 2>/dev/null | tr '\\033' E"
                        .into(),
                ],
                cwd: std::env::temp_dir(),
                env: vec![],
                scrub_env: &[],
                cols: DEFAULT_COLS,
                rows: DEFAULT_ROWS,
            },
        )
        .unwrap();
        let mut rx = session.events.subscribe();
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                match rx.recv().await {
                    Ok(PtyEvent::Exited { .. }) => break,
                    Ok(_) => continue,
                    Err(e) => panic!("event stream ended: {e}"),
                }
            }
        })
        .await
        .expect("child reads its reply and exits within 10s");
        let (_, bytes) = session.snapshot(None);
        let out = String::from_utf8_lossy(&bytes);
        assert!(out.contains("REPLY:E[3;7R"), "child saw: {out:?}");
    }

    /// The kill escalation must reach an agent the login shell forked into
    /// a job group of its own, not just the PTY leader's group.
    #[test]
    fn process_groups_cover_a_job_the_shell_forked_into_its_own_group() {
        // shell 20 leads its session (group 20); the agent 21 is a job in
        // group 21 with a worker 22; 30 is another session, 99 unrelated.
        let table = "\
 20    10    20
 21    20    21
 22    21    21
 30    10    30
 99     1    99
";
        assert_eq!(process_groups_in_table(table, 20), vec![20, 21]);
        // A failed sweep still names the leader's own group.
        assert_eq!(process_groups_in_table("", 20), vec![20]);
    }

    /// The DELETE CHECK spares a worktree's own sessions and everything
    /// under them — they are counted already, and go down with it.
    #[test]
    fn subtree_pids_take_each_root_and_everything_below_it() {
        let table = "\
 20    10    20
 21    20    21
 22    21    21
 30    10    30
 31    30    30
 99     1    99
";
        let mut got: Vec<u32> = subtree_pids_in_table(table, &[20, 30])
            .into_iter()
            .collect();
        got.sort_unstable();
        assert_eq!(got, vec![20, 21, 22, 30, 31]);
        assert_eq!(
            subtree_pids_in_table("", &[20])
                .into_iter()
                .collect::<Vec<_>>(),
            vec![20]
        );
        assert_eq!(children_in_table(table, 10), vec![20, 30]);
        assert!(children_in_table(table, 22).is_empty());
    }

    /// The reaper's "still working?" question is whether anything under the
    /// agent leads a session of its own: a backgrounded Bash call does
    /// (Claude spawns it detached), an MCP server or a shell job does not.
    #[test]
    fn detached_job_is_a_session_leader_below_the_agent() {
        // Agent 20 leads its PTY session and is its foreground job (`+`).
        // Its MCP server 21 shares that group; 22 is a helper in a group of
        // its own but the same session (Codex's code-mode host). 30 is
        // another session entirely, 99 unrelated.
        let idle = "\
 20    10    20  Ss+
 21    20    20  S+
 22    20    22  S
 30    10    30  Ss+
 99     1    99  S
";
        assert!(!detached_job_in_table(idle, 20));
        // A backgrounded Bash call: shell 23 started its own session (`s`)
        // and runs sleep 24 inside it.
        let busy = format!("{idle} 23    20    23  Ss\n 24    23    23  S\n");
        assert!(detached_job_in_table(&busy, 20));
        // Under a login shell that forked the agent as a job: 20 is bash,
        // 21 the agent in a group of its own but not a session of its own,
        // 25 its detached Bash call.
        let wrapped = "\
 20    10    20  Ss
 21    20    21  S+
 25    21    25  Ss
";
        assert!(detached_job_in_table(wrapped, 20));
        assert!(!detached_job_in_table(
            " 20    10    20  Ss\n 21    20    21  S+\n",
            20
        ));
        // Another session's job, a missing root, an empty sweep: no work.
        assert!(!detached_job_in_table(idle, 30));
        assert!(!detached_job_in_table(idle, 40));
        assert!(!detached_job_in_table("", 20));
    }
}
