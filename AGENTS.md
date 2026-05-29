# AGENTS.md

## Scope

These notes are for agents working in this repository.

## Repo Truth

- Treat this repository as the source of truth.
- The tmux plugin used at runtime is typically a TPM-managed clone under `~/.tmux/plugins/ss`.
- Do not make durable fixes in `~/.tmux/plugins/ss`; those are deployment artifacts and can be overwritten by TPM updates.

## Where Launch Behavior Lives

- Tmux launch behavior is defined in `scripts/ss.tmux`.
- The tmux key binding is registered by `ss.tmux`.
- The Rust app in `src/` is the viewer itself; it does not create tmux splits/windows/popups.
- If `ss` opens in the wrong tmux surface, inspect the wrapper script before changing Rust code.

## Fast Debug Path

When a user reports popup/split/pane/window launch problems:

1. Check which script tmux is executing with `tmux list-keys`.
2. Check `tmux show -gqv @ss_launch_mode`.
3. Compare the local repo revision with the TPM clone revision.
4. Fix `scripts/ss.tmux` in this repo.
5. Tell the user local repo edits do not affect TPM until they push and update plugins, unless tmux is temporarily pointed at the local repo.

## Safe Validation

- Prefer validating by temporarily pointing tmux at this local repo, or by explaining that the user must push and run `prefix + U`.
- Avoid patching `~/.tmux/plugins/ss` unless the user explicitly wants a temporary live hotfix.

## Current Launch Modes

- `pane`: reuse the current pane via `tmux respawn-pane -k`
- `window`: open a new tmux window
- `popup`: open the viewer in a tmux popup via `tmux display-popup`

## Change Style

- Keep fixes minimal and wrapper-focused for tmux launch issues.
- Update `README.md` when changing user-facing tmux options or defaults.
