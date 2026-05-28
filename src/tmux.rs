use std::process::Command;

use anyhow::{Context, Result};

#[derive(Clone, Debug, Default)]
pub struct TmuxContext {
    socket: String,
    pane_id: String,
    window_id: String,
    session_id: String,
}

impl TmuxContext {
    pub fn detect() -> Self {
        Self {
            socket: socket_path(),
            pane_id: std::env::var("SS_TMUX_PANE_ID").unwrap_or_default(),
            window_id: std::env::var("SS_TMUX_WINDOW_ID").unwrap_or_default(),
            session_id: std::env::var("SS_TMUX_SESSION_ID").unwrap_or_default(),
        }
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
        Ok(TmuxActive {
            pane_active: parts.next().unwrap_or("0") == "1",
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
    pub pane_active: bool,
    pub window_active: bool,
    pub session_attached: bool,
}

impl TmuxActive {
    pub fn visible(self) -> bool {
        self.pane_active && self.window_active && self.session_attached
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
