#!/usr/bin/env bash
set -euo pipefail

CURRENT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
KEY_BIND="$(tmux show -gqv @ss_key || true)"

if [[ -z "${KEY_BIND}" ]]; then
  KEY_BIND="S"
fi

tmux bind-key "${KEY_BIND}" run-shell "${CURRENT_DIR}/scripts/ss.tmux"
