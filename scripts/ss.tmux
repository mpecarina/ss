#!/usr/bin/env bash
set -euo pipefail

CURRENT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${CURRENT_DIR}/.." && pwd)"

BIN_PATH="$(tmux show -gqv @ss_bin || true)"
LAUNCH_MODE="$(tmux show -gqv @ss_launch_mode || true)"
PROBE_MODE="$(tmux show -gqv @ss_probe || true)"
TMUX_SOCKET="${TMUX%%,*}"
PANE_ID="$(tmux display-message -p '#{pane_id}' || true)"
WINDOW_ID="$(tmux display-message -p '#{window_id}' || true)"
SESSION_ID="$(tmux display-message -p '#{session_id}' || true)"
RUNTIME_ID="ss-$(date +%s)-$$"

if [[ -z "${BIN_PATH}" ]]; then
  BIN_PATH="${REPO_ROOT}/bin/ss"
fi
if [[ "${BIN_PATH}" == "~/"* ]]; then
  BIN_PATH="${HOME}/${BIN_PATH:2}"
fi
if [[ -z "${LAUNCH_MODE}" ]]; then
  LAUNCH_MODE="popup"
fi

STAMP_FILE="${BIN_PATH}.commit"
CURRENT_COMMIT="$(cd "${REPO_ROOT}" && git rev-parse HEAD 2>/dev/null || echo unknown)"
NEEDS_BUILD=0

if [[ ! -x "${BIN_PATH}" ]]; then
  NEEDS_BUILD=1
elif [[ ! -f "${STAMP_FILE}" ]]; then
  NEEDS_BUILD=1
elif [[ "$(cat "${STAMP_FILE}" 2>/dev/null)" != "${CURRENT_COMMIT}" ]]; then
  NEEDS_BUILD=1
fi

if [[ "${NEEDS_BUILD}" -eq 1 ]]; then
  tmux display-message "ss: building..."
  if (cd "${REPO_ROOT}" && cargo build --release) >/dev/null 2>&1; then
    mkdir -p "$(dirname "${BIN_PATH}")"
    cp "${REPO_ROOT}/target/release/ss" "${BIN_PATH}"
    echo "${CURRENT_COMMIT}" > "${STAMP_FILE}"
  else
    tmux display-message -d 5000 "ss: build failed — run 'cargo build --release' manually"
    exit 1
  fi
fi

PANE_PATH="$(tmux display-message -p '#{pane_current_path}' || true)"

CMD=(
  env
  "SS_IMAGE_PROBE=${PROBE_MODE}"
  "SS_TMUX_SOCKET=${TMUX_SOCKET}"
  "SS_TMUX_PANE_ID=${PANE_ID}"
  "SS_TMUX_WINDOW_ID=${WINDOW_ID}"
  "SS_TMUX_SESSION_ID=${SESSION_ID}"
  "SS_LAUNCH_MODE=${LAUNCH_MODE}"
  "SS_RUNTIME_ID=${RUNTIME_ID}"
  "${BIN_PATH}"
  "${PANE_PATH}"
)

if [[ "${LAUNCH_MODE}" == "popup" ]]; then
  tmux display-popup -E -w 90% -h 85% -- "${CMD[@]}"
  exit 0
fi

tmux new-window -n "ss" -- "${CMD[@]}"
