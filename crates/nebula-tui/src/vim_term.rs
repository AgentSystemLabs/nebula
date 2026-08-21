//! Embedded editor modal: a local PTY child (vim) rendered inside the TUI.
//!
//! Unlike agent/terminal sessions (daemon-owned PTYs reached over IPC), the
//! editor is spawned in-process: it's a short-lived affordance of the
//! find-in-files overlay, needs no persistence or reattach, and dies with
//! the client. Output flows reader thread → mpsc → the main loop, which
//! feeds the vt100 parser here (the daemon's `PtySession` shape, minus the
//! ring buffer and broadcast).

use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use ratatui::layout::Rect;
use std::io::{Read, Write};
use std::path::Path;
use tokio::sync::mpsc::UnboundedSender;

/// Reader-thread → main-loop messages. `generation` stamps which spawn they
/// belong to, so bytes from a closed editor can't bleed into a new one.
#[derive(Debug)]
pub enum VimEvent {
    Output { generation: u64, data: Vec<u8> },
    Exited { generation: u64 },
}

pub struct VimTerm {
    pub generation: u64,
    pub parser: vt100::Parser,
    /// Size the parser (and PTY) currently uses.
    pub cols: u16,
    pub rows: u16,
    /// "path:line" for the modal title.
    pub title: String,
    /// Rendered inside the tree browser's preview pane instead of the
    /// centered modal (set by the tree browser's Enter).
    pub embedded: bool,
    /// Inner rect from the last draw; `sync_vim_size` resizes to it.
    pub area: Rect,
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    killer: Box<dyn ChildKiller + Send + Sync>,
}

impl VimTerm {
    /// Spawn `editor +<line> <file>` in the checkout. `Err` is a user-facing
    /// flash message.
    #[allow(clippy::too_many_arguments)]
    pub fn spawn_editor(
        editor: &str,
        root: &Path,
        file: &str,
        line: u64,
        cols: u16,
        rows: u16,
        generation: u64,
        tx: UnboundedSender<VimEvent>,
    ) -> Result<Self, String> {
        let title = format!("{file}:{line}");
        Self::spawn_cmd(
            editor,
            &[format!("+{line}"), file.to_string()],
            root,
            title,
            cols,
            rows,
            generation,
            tx,
        )
    }

    /// Editor-agnostic spawn (tests use a shell here).
    #[allow(clippy::too_many_arguments)]
    pub fn spawn_cmd(
        program: &str,
        args: &[String],
        cwd: &Path,
        title: String,
        cols: u16,
        rows: u16,
        generation: u64,
        tx: UnboundedSender<VimEvent>,
    ) -> Result<Self, String> {
        let cols = cols.max(2);
        let rows = rows.max(2);
        let pair = native_pty_system()
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| format!("openpty failed: {e}"))?;

        let mut cmd = CommandBuilder::new(program);
        cmd.args(args);
        cmd.cwd(cwd);
        cmd.env("TERM", "xterm-256color");

        let mut child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| format!("failed to launch {program}: {e}"))?;
        drop(pair.slave);

        let killer = child.clone_killer();
        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| format!("pty reader: {e}"))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|e| format!("pty writer: {e}"))?;

        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let data = buf[..n].to_vec();
                        if tx.send(VimEvent::Output { generation, data }).is_err() {
                            break; // main loop gone
                        }
                    }
                }
            }
            let _ = child.wait(); // reap
            let _ = tx.send(VimEvent::Exited { generation });
        });

        Ok(Self {
            generation,
            parser: vt100::Parser::new(rows, cols, 0),
            cols,
            rows,
            title,
            embedded: false,
            area: Rect::default(),
            master: pair.master,
            writer,
            killer,
        })
    }

    /// Feed reader-thread output into the emulator.
    pub fn process(&mut self, data: &[u8]) {
        self.parser.process(data);
    }

    /// Encoded keystrokes (and bracketed pastes) go straight to the child.
    pub fn input(&mut self, data: &[u8]) {
        // A write error means the child died; the Exited event closes us.
        let _ = self
            .writer
            .write_all(data)
            .and_then(|_| self.writer.flush());
    }

    /// Keep the PTY and parser sized to the drawn modal.
    pub fn resize(&mut self, cols: u16, rows: u16) {
        let (cols, rows) = (cols.max(2), rows.max(2));
        if (self.cols, self.rows) == (cols, rows) {
            return;
        }
        self.cols = cols;
        self.rows = rows;
        self.parser.set_size(rows, cols);
        let _ = self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });
    }

    /// Force-close (the Ctrl+Q hatch); the reader thread reaps the child.
    pub fn kill(&mut self) {
        let _ = self.killer.kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    async fn recv_until(
        rx: &mut tokio::sync::mpsc::UnboundedReceiver<VimEvent>,
        term: &mut VimTerm,
        mut done: impl FnMut(&VimTerm, &VimEvent) -> bool,
    ) {
        let ok = tokio::time::timeout(Duration::from_secs(10), async {
            while let Some(ev) = rx.recv().await {
                if let VimEvent::Output { data, .. } = &ev {
                    term.process(data);
                }
                if done(term, &ev) {
                    return;
                }
            }
            panic!("event channel closed early");
        })
        .await;
        assert!(
            ok.is_ok(),
            "timed out; screen:\n{}",
            term.parser.screen().contents()
        );
    }

    #[tokio::test]
    async fn output_reaches_parser_and_kill_exits() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut term = VimTerm::spawn_cmd(
            "/bin/sh",
            &["-c".into(), "printf 'VIM_MODAL_TEST'; sleep 30".into()],
            dir.path(),
            "test".into(),
            80,
            24,
            1,
            tx,
        )
        .unwrap();

        recv_until(&mut rx, &mut term, |t, _| {
            t.parser.screen().contents().contains("VIM_MODAL_TEST")
        })
        .await;

        term.kill();
        recv_until(&mut rx, &mut term, |_, ev| {
            matches!(ev, VimEvent::Exited { generation: 1 })
        })
        .await;
    }

    #[tokio::test]
    async fn input_reaches_the_child() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut term = VimTerm::spawn_cmd(
            "/bin/sh",
            &["-c".into(), "read line; printf \"GOT:$line\"".into()],
            dir.path(),
            "test".into(),
            80,
            24,
            7,
            tx,
        )
        .unwrap();

        term.input(b"hello\r");
        recv_until(&mut rx, &mut term, |t, _| {
            t.parser.screen().contents().contains("GOT:hello")
        })
        .await;
        // The script ends after one line: the exit must be reported with the
        // spawn's generation stamp.
        recv_until(&mut rx, &mut term, |_, ev| {
            matches!(ev, VimEvent::Exited { generation: 7 })
        })
        .await;
    }

    #[tokio::test]
    async fn spawn_failure_is_a_message_not_a_panic() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let err = VimTerm::spawn_cmd(
            "/nonexistent-editor-binary",
            &[],
            dir.path(),
            "test".into(),
            80,
            24,
            1,
            tx,
        )
        .map(|_| ())
        .unwrap_err();
        assert!(err.contains("failed to launch"), "{err}");
    }
}
