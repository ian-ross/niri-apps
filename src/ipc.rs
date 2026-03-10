use anyhow::{Context, Result, bail};
use niri_ipc::{Action, Reply, Request, Response, Window};
use std::collections::HashSet;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::thread::sleep;
use std::time::{Duration, Instant};

fn socket_path() -> Result<String> {
    std::env::var("NIRI_SOCKET")
        .context("NIRI_SOCKET environment variable is not set; is niri running?")
}

fn send_request(request: &Request) -> Result<Response> {
    let path = socket_path()?;
    let mut stream =
        UnixStream::connect(&path).with_context(|| format!("connecting to niri socket {path}"))?;

    let msg = serde_json::to_string(request).context("serialising request")?;
    stream
        .write_all(msg.as_bytes())
        .context("writing request to socket")?;
    stream.write_all(b"\n").context("writing newline")?;

    let mut reader = BufReader::new(&stream);
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .context("reading response from socket")?;

    let reply: Reply = serde_json::from_str(&line).context("deserialising response")?;
    reply.map_err(|e| anyhow::anyhow!("niri error: {e}"))
}

/// Focus the workspace with the given index (1-based).
pub fn focus_workspace(index: u8) -> Result<()> {
    let request = Request::Action(Action::FocusWorkspace {
        reference: niri_ipc::WorkspaceReferenceArg::Index(index),
    });
    let response = send_request(&request)?;
    match response {
        Response::Handled => Ok(()),
        other => bail!("unexpected response to FocusWorkspace: {other:?}"),
    }
}

/// Center all visible columns on the focused workspace.
pub fn center_visible_columns() -> Result<()> {
    let request = Request::Action(Action::CenterVisibleColumns {});
    let response = send_request(&request)?;
    match response {
        Response::Handled => Ok(()),
        other => bail!("unexpected response to CenterVisibleColumns: {other:?}"),
    }
}

/// Return the list of all open windows.
pub fn list_windows() -> Result<Vec<Window>> {
    let response = send_request(&Request::Windows)?;
    match response {
        Response::Windows(windows) => Ok(windows),
        other => bail!("unexpected response to Windows: {other:?}"),
    }
}

/// Poll the window list until a window whose ID is not in `known_ids` appears,
/// then return that window's ID. Times out after 30 seconds.
pub fn wait_for_new_window(known_ids: &HashSet<u64>) -> Result<u64> {
    let timeout = Duration::from_secs(30);
    let poll_interval = Duration::from_millis(200);
    let start = Instant::now();

    loop {
        if start.elapsed() > timeout {
            bail!("timed out waiting for new window to appear");
        }
        for window in list_windows()? {
            if !known_ids.contains(&window.id) {
                return Ok(window.id);
            }
        }
        sleep(poll_interval);
    }
}

/// Consume or expel the focused window to the left (merges it into the
/// column to its left).
pub fn consume_or_expel_window_left() -> Result<()> {
    let request = Request::Action(Action::ConsumeOrExpelWindowLeft { id: None });
    let response = send_request(&request)?;
    match response {
        Response::Handled => Ok(()),
        other => bail!("unexpected response to ConsumeOrExpelWindowLeft: {other:?}"),
    }
}
