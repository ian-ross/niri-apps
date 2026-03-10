use anyhow::{Context, Result, bail};
use niri_ipc::{Action, Request, Response};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;

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

    let response: Response = serde_json::from_str(&line).context("deserialising response")?;
    Ok(response)
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
