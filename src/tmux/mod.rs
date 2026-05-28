#![allow(dead_code)]

use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};

#[derive(Clone, Debug, Default)]
pub struct TmuxRuntime {
    pub socket: String,
    pub pane_id: String,
    pub window_id: String,
    pub session_id: String,
    pub launch_mode: LaunchMode,
    pub runtime_id: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LaunchMode {
    #[default]
    Popup,
    Window,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct VisibilityState {
    pub session_attached: bool,
    pub window_active: bool,
    pub pane_active: bool,
    pub pane_top: u16,
    pub pane_left: u16,
}

impl VisibilityState {
    pub fn safe_to_draw_images(self) -> bool {
        self.session_attached && self.window_active && self.pane_active
    }
}

impl TmuxRuntime {
    pub fn detect() -> Self {
        let socket = std::env::var("SS_TMUX_SOCKET").unwrap_or_else(|_| socket_path());
        let pane_id = current_format(&socket, "#{pane_id}")
            .or_else(|| std::env::var("SS_TMUX_PANE_ID").ok())
            .unwrap_or_default();
        let window_id = current_format(&socket, "#{window_id}")
            .or_else(|| std::env::var("SS_TMUX_WINDOW_ID").ok())
            .unwrap_or_default();
        let session_id = current_format(&socket, "#{session_id}")
            .or_else(|| std::env::var("SS_TMUX_SESSION_ID").ok())
            .unwrap_or_default();
        let launch_mode = match std::env::var("SS_LAUNCH_MODE").unwrap_or_default().as_str() {
            "window" => LaunchMode::Window,
            _ => LaunchMode::Popup,
        };
        let runtime_id = std::env::var("SS_RUNTIME_ID").unwrap_or_else(|_| generate_runtime_id());
        Self {
            socket,
            pane_id,
            window_id,
            session_id,
            launch_mode,
            runtime_id,
        }
    }

    pub fn in_tmux(&self) -> bool {
        !self.socket.is_empty()
            || std::env::var("TMUX")
                .map(|v| !v.trim().is_empty())
                .unwrap_or(false)
    }

    pub fn poll_visibility(&self) -> Result<VisibilityState> {
        if !self.in_tmux() || self.pane_id.trim().is_empty() {
            return Ok(VisibilityState {
                session_attached: true,
                window_active: true,
                pane_active: true,
                pane_top: 0,
                pane_left: 0,
            });
        }

        let target = self.output(&[
            "display-message",
            "-p",
            "-t",
            self.pane_id.as_str(),
            "#{session_attached}|#{pane_top}|#{pane_left}",
        ])?;
        let active = self.output(&[
            "display-message",
            "-p",
            "#{session_id}|#{window_id}|#{pane_id}",
        ])?;

        let mut target_parts = target.trim().split('|');
        let mut active_parts = active.trim().split('|');
        let active_session_id = active_parts.next().unwrap_or_default();
        let active_window_id = active_parts.next().unwrap_or_default();
        let active_pane_id = active_parts.next().unwrap_or_default();
        Ok(VisibilityState {
            session_attached: target_parts.next().unwrap_or("0") != "0"
                && active_session_id == self.session_id,
            window_active: active_window_id == self.window_id,
            pane_active: active_pane_id == self.pane_id,
            pane_top: target_parts
                .next()
                .unwrap_or("0")
                .parse()
                .unwrap_or_default(),
            pane_left: target_parts
                .next()
                .unwrap_or("0")
                .parse()
                .unwrap_or_default(),
        })
    }

    pub fn wrap_passthrough(&self, seq: &str) -> String {
        if !self.in_tmux() || seq.is_empty() {
            return seq.to_string();
        }
        let escaped = seq.replace('\x1b', "\x1b\x1b");
        format!("\x1bPtmux;{}\x1b\\", escaped)
    }

    pub fn client_terminal(&self) -> Option<String> {
        if !self.in_tmux() {
            return None;
        }

        let term = self
            .output(&[
                "display-message",
                "-p",
                "#{client_termname}|#{client_termtype}",
            ])
            .ok()?;
        let combined = term.trim().to_lowercase();
        if combined.is_empty() || combined == "|" {
            None
        } else {
            Some(combined)
        }
    }

    pub fn output(&self, args: &[&str]) -> Result<String> {
        let mut cmd = Command::new("tmux");
        if !self.socket.is_empty() {
            cmd.arg("-S").arg(&self.socket);
        }
        cmd.args(args);
        let output = cmd
            .output()
            .with_context(|| format!("tmux {}", args.join(" ")))?;
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
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

fn current_format(socket: &str, format: &str) -> Option<String> {
    let mut cmd = Command::new("tmux");
    if !socket.is_empty() {
        cmd.arg("-S").arg(socket);
    }
    cmd.args(["display-message", "-p", format]);
    let output = cmd.output().ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if value.is_empty() { None } else { Some(value) }
}

fn generate_runtime_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!("ss-{nanos}")
}
