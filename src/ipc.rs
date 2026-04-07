use anyhow::{Context, Result, bail};
use niri_ipc::{Action, ColumnDisplay, Event, Reply, Request, Response, SizeChange, Workspace};
use std::collections::{HashMap, HashSet};
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

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

/// Focus the first column on the focused workspace.
pub fn focus_column_first() -> Result<()> {
    let request = Request::Action(Action::FocusColumnFirst {});
    let response = send_request(&request)?;
    match response {
        Response::Handled => Ok(()),
        other => bail!("unexpected response to FocusColumnFirst: {other:?}"),
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

/// Set the display mode of the focused column.
pub fn set_column_display(display: ColumnDisplay) -> Result<()> {
    let request = Request::Action(Action::SetColumnDisplay { display });
    let response = send_request(&request)?;
    match response {
        Response::Handled => Ok(()),
        other => bail!("unexpected response to SetColumnDisplay: {other:?}"),
    }
}

/// Set the width of the focused column as a proportion of the working area.
pub fn set_column_width(change: SizeChange) -> Result<()> {
    let request = Request::Action(Action::SetColumnWidth { change });
    let response = send_request(&request)?;
    match response {
        Response::Handled => Ok(()),
        other => bail!("unexpected response to SetColumnWidth: {other:?}"),
    }
}

/// A persistent connection to the niri IPC event stream.
///
/// On connection niri sends an initial state snapshot via `WorkspacesChanged`
/// and `WindowsChanged` events before any incremental events.  All subsequent
/// state changes arrive as individual events in compositor order.
///
/// Because requesting `EventStream` stops niri from reading further requests
/// on the *same* socket, all IPC actions (focus, set width, …) must be sent on
/// separate connections — that is already the case for `send_request`, which
/// opens a fresh socket per call.
pub struct EventStream {
    reader: BufReader<UnixStream>,
    /// Id of the currently-focused workspace (updated from `WorkspaceActivated`).
    focused_workspace_id: Option<u64>,
    /// Map from workspace index to stable workspace id (updated from
    /// `WorkspacesChanged`).  Niri creates new workspaces on demand when a
    /// `FocusWorkspace` action targets a previously non-existent index, so
    /// this map must be kept current from events rather than read once at
    /// startup.
    workspace_ids: HashMap<u8, u64>,
}

impl EventStream {
    /// Connect to niri, request the event stream, and consume the initial
    /// state snapshot.  Returns the stream together with:
    /// - the initial workspace list (used to compute the starting workspace
    ///   index), and
    /// - the set of window ids that are already open (used to detect newly
    ///   spawned windows).
    pub fn connect() -> Result<(Self, Vec<Workspace>, HashSet<u64>)> {
        let path = socket_path()?;
        let stream = UnixStream::connect(&path)
            .with_context(|| format!("connecting to niri event stream socket {path}"))?;
        // All blocking reads on this socket are bounded by this timeout.
        stream
            .set_read_timeout(Some(Duration::from_secs(30)))
            .context("setting read timeout on event stream socket")?;
        let mut reader = BufReader::new(stream);

        // Request the event stream.
        let msg = serde_json::to_string(&Request::EventStream)
            .context("serialising EventStream request")?;
        reader
            .get_mut()
            .write_all(msg.as_bytes())
            .context("writing EventStream request")?;
        reader
            .get_mut()
            .write_all(b"\n")
            .context("writing newline")?;

        // Niri replies with Reply::Ok(Response::Handled) before sending events.
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .context("reading EventStream response")?;
        let reply: Reply =
            serde_json::from_str(&line).context("deserialising EventStream response")?;
        reply.map_err(|e| anyhow::anyhow!("niri error starting event stream: {e}"))?;

        // Consume the initial snapshot.  Niri always sends `WorkspacesChanged`
        // and `WindowsChanged` (among other state events) before any
        // incremental events.
        let mut initial_workspaces: Option<Vec<Workspace>> = None;
        let mut initial_window_ids: Option<HashSet<u64>> = None;
        let mut focused_workspace_id: Option<u64> = None;
        let mut workspace_ids: HashMap<u8, u64> = HashMap::new();

        while initial_workspaces.is_none() || initial_window_ids.is_none() {
            line.clear();
            reader.read_line(&mut line).context("reading initial event")?;
            if line.trim().is_empty() {
                bail!("niri IPC event stream closed during initial state");
            }
            let event: Event =
                serde_json::from_str(&line).context("deserialising initial event")?;
            match event {
                Event::WorkspacesChanged { workspaces } => {
                    focused_workspace_id =
                        workspaces.iter().find(|ws| ws.is_focused).map(|ws| ws.id);
                    workspace_ids =
                        workspaces.iter().map(|ws| (ws.idx, ws.id)).collect();
                    initial_workspaces = Some(workspaces);
                }
                Event::WindowsChanged { windows } => {
                    initial_window_ids =
                        Some(windows.into_iter().map(|w| w.id).collect());
                }
                _ => {}
            }
        }

        Ok((
            Self {
                reader,
                focused_workspace_id,
                workspace_ids,
            },
            initial_workspaces.unwrap(),
            initial_window_ids.unwrap(),
        ))
    }

    /// Read one event from the stream, updating internal state as a side
    /// effect.  Returns an error if the stream closes or times out.
    fn read_next_event(&mut self) -> Result<Event> {
        let mut line = String::new();
        match self.reader.read_line(&mut line) {
            Ok(0) => bail!("niri IPC event stream closed unexpectedly"),
            Ok(_) => {}
            Err(e)
                if e.kind() == io::ErrorKind::WouldBlock
                    || e.kind() == io::ErrorKind::TimedOut =>
            {
                bail!("timed out waiting for niri IPC event");
            }
            Err(e) => return Err(anyhow::anyhow!(e)).context("reading event from stream"),
        }
        let event: Event = serde_json::from_str(&line).context("deserialising event")?;

        // Keep workspace state current so lookups in the wait helpers are
        // always up to date.
        match &event {
            Event::WorkspaceActivated { id, focused: true } => {
                self.focused_workspace_id = Some(*id);
            }
            Event::WorkspacesChanged { workspaces } => {
                self.workspace_ids =
                    workspaces.iter().map(|ws| (ws.idx, ws.id)).collect();
            }
            _ => {}
        }
        Ok(event)
    }

    /// Block until the workspace at `ws_index` is confirmed as focused by a
    /// `WorkspaceActivated` event, then return its stable id.
    ///
    /// Returns immediately if the workspace is already focused according to
    /// the tracked state.  The workspace may not exist yet when this is called
    /// (niri creates it on demand in response to `FocusWorkspace`); in that
    /// case the preceding `WorkspacesChanged` event — which `read_next_event`
    /// processes before returning it — will have populated `workspace_ids`
    /// with the new entry before the `WorkspaceActivated` event is seen.
    pub fn wait_for_workspace_focus(&mut self, ws_index: u8) -> Result<u64> {
        loop {
            if let Some(&ws_id) = self.workspace_ids.get(&ws_index)
                && self.focused_workspace_id == Some(ws_id)
            {
                return Ok(ws_id);
            }
            self.read_next_event()?;
        }
    }

    /// Block until an [`Event::WindowLayoutsChanged`] event arrives that
    /// includes the given window id, then return.
    ///
    /// This is used after a `SetColumnWidth` action to wait for the
    /// column-width change to propagate through the compositor's layout
    /// engine and the Wayland configure/commit round-trip before attempting
    /// to center visible columns.  Without this wait, `CenterVisibleColumns`
    /// can run before the window has committed its new buffer, and a
    /// subsequent viewport adjustment by Niri (to keep columns in view after
    /// the commit) will undo the centering.
    pub fn wait_for_window_layout_change(&mut self, window_id: u64) -> Result<()> {
        loop {
            let event = self.read_next_event()?;
            if let Event::WindowLayoutsChanged { changes } = &event {
                if changes.iter().any(|(id, _)| *id == window_id) {
                    return Ok(());
                }
            }
        }
    }

    /// Block until a window that is not in `known_ids` appears on the
    /// workspace identified by `workspace_id`, then return the new window's
    /// id.
    pub fn wait_for_new_window(
        &mut self,
        known_ids: &HashSet<u64>,
        workspace_id: u64,
    ) -> Result<u64> {
        loop {
            let event = self.read_next_event()?;
            if let Event::WindowOpenedOrChanged { window } = event
                && !known_ids.contains(&window.id)
                && window.workspace_id == Some(workspace_id)
                && !window.is_floating
            {
                return Ok(window.id);
            }
        }
    }
}
