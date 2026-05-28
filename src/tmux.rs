use std::process::Command;

use anyhow::{Context, Result};

#[derive(Clone, Debug, Default)]
pub struct TmuxContext {
    socket: String,
    pane_id: String,
}

impl TmuxContext {
    pub fn detect() -> Self {
        let socket = socket_path();
        let pane_id = current_pane_id(&socket)
            .or_else(|| std::env::var("SS_TMUX_PANE_ID").ok())
            .unwrap_or_default();
        Self { socket, pane_id }
    }

    pub fn in_tmux(&self) -> bool {
        !self.socket.is_empty() || std::env::var("TMUX").map(|v| !v.trim().is_empty()).unwrap_or(false)
    }

    pub fn pane_id(&self) -> &str {
        self.pane_id.trim()
    }

    pub fn poll_active(&self) -> Result<TmuxActive> {
        if !self.in_tmux() || self.pane_id().is_empty() {
            return Ok(TmuxActive::default());
        }

        let output = self.output(&[
            "display-message",
            "-p",
            "-t",
            self.pane_id(),
            "#{?pane_active,1,0}|#{window_active}|#{session_attached}",
        ])?;
        let mut parts = output.trim().split('|');
        let _pane_active = parts.next().unwrap_or("0") == "1";
        Ok(TmuxActive {
            window_active: parts.next().unwrap_or("0") == "1",
            session_attached: parts.next().unwrap_or("0") != "0",
        })
    }

    pub fn output(&self, args: &[&str]) -> Result<String> {
        let mut cmd = Command::new("tmux");
        if !self.socket.is_empty() {
            cmd.arg("-S").arg(&self.socket);
        }
        cmd.args(args);
        let output = cmd.output().with_context(|| format!("tmux {}", args.join(" ")))?;
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok(stdout)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct TmuxActive {
    pub window_active: bool,
    pub session_attached: bool,
}

impl TmuxActive {
    pub fn visible(self) -> bool {
        // Popup panes can report pane_active inconsistently even while visible.
        // Window/session visibility is the more stable signal for cleanup.
        self.window_active && self.session_attached
    }
}

fn socket_path() -> String {
    let tmux = std::env::var("TMUX").unwrap_or_default();
    let trimmed = tmux.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    match trimmed.find(',') {
        Some(index) => trimmed[..index].to_string(),
        None => trimmed.to_string(),
    }
}

fn current_pane_id(socket: &str) -> Option<String> {
    let mut cmd = Command::new("tmux");
    if !socket.is_empty() {
        cmd.arg("-S").arg(socket);
    }
    cmd.args(["display-message", "-p", "#{pane_id}"]);
    let output = cmd.output().ok()?;
    if !output.status.success() {
        return None;
    }
    let pane = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if pane.is_empty() {
        None
    } else {
        Some(pane)
    }
}
