//! Throwaway phase-2 client: raw-mode passthrough to one daemon session.
//! Proves the detach/reattach story before the real TUI exists.
//! Detach with Ctrl+\.

use crate::ipc;
use anyhow::Result;
use nebula_core::codec::{read_frame, write_frame};
use nebula_core::{ClientRequest, ServerEvent, SessionRef, TerminalId};
use std::io::Write;
use tokio::io::AsyncReadExt;

const DETACH_BYTE: u8 = 0x1c; // Ctrl+\

pub async fn run(name: &str) -> Result<()> {
    let conn = ipc::connect_or_spawn().await?;
    let (read_half, mut write_half) = conn.stream.into_split();
    let mut reader = tokio::io::BufReader::new(read_half);

    let sref = SessionRef::Terminal(TerminalId(format!("scratch-{name}")));
    let (cols, rows) = crossterm::terminal::size()?;
    write_frame(
        &mut write_half,
        &ClientRequest::Attach {
            session: sref.clone(),
            from_seq: None,
            cols,
            rows,
        },
    )
    .await?;

    crossterm::terminal::enable_raw_mode()?;
    let result = passthrough(&mut reader, &mut write_half, &sref, cols, rows).await;
    crossterm::terminal::disable_raw_mode()?;
    println!();
    result
}

async fn passthrough(
    reader: &mut (impl tokio::io::AsyncRead + Unpin),
    writer: &mut (impl tokio::io::AsyncWrite + Unpin),
    sref: &SessionRef,
    mut cols: u16,
    mut rows: u16,
) -> Result<()> {
    let mut stdin = tokio::io::stdin();
    let mut stdin_buf = [0u8; 1024];
    let mut stdout = std::io::stdout();
    // Cheap SIGWINCH stand-in for the throwaway client: poll terminal size.
    let mut size_tick = tokio::time::interval(std::time::Duration::from_millis(500));

    loop {
        tokio::select! {
            frame = read_frame::<ServerEvent, _>(reader) => {
                match frame? {
                    Some(ServerEvent::Scrollback { data, .. }) | Some(ServerEvent::Output { data, .. }) => {
                        stdout.write_all(&data)?;
                        stdout.flush()?;
                    }
                    Some(ServerEvent::SessionExited { exit_code, .. }) => {
                        eprintln!("\r\n[nebula] session exited ({exit_code:?})\r");
                        return Ok(());
                    }
                    Some(_) => {}
                    None => {
                        eprintln!("\r\n[nebula] daemon closed the connection\r");
                        return Ok(());
                    }
                }
            }
            n = stdin.read(&mut stdin_buf) => {
                let n = n?;
                if n == 0 {
                    return Ok(());
                }
                if stdin_buf[..n].contains(&DETACH_BYTE) {
                    eprintln!("\r\n[nebula] detached\r");
                    write_frame(writer, &ClientRequest::Detach { session: sref.clone() }).await?;
                    return Ok(());
                }
                write_frame(writer, &ClientRequest::Input {
                    session: sref.clone(),
                    data: stdin_buf[..n].to_vec(),
                }).await?;
            }
            _ = size_tick.tick() => {
                if let Ok((c, r)) = crossterm::terminal::size() {
                    if (c, r) != (cols, rows) {
                        cols = c;
                        rows = r;
                        write_frame(writer, &ClientRequest::Resize {
                            session: sref.clone(), cols, rows,
                        }).await?;
                    }
                }
            }
        }
    }
}
