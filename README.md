# ss

tmux-aware markdown slideshow with kitty/ghostty image rendering.

## Build

```sh
make build
./bin/ss .
./bin/ss --watch .
./bin/ss -w .
```

Run directly:

```sh
make run ARGS=./examples
```

## Tmux Plugin Usage

Recommended: install through TPM so `prefix + U` updates the repo and the next
launch rebuilds `target/release/ss` automatically when the git commit changed.

Add to your `~/.tmux.conf`:

```tmux
set -g allow-passthrough on
set -g @plugin 'mpecarina/ss'

# optional
set -g @ss_launch_mode 'pane'
set -g @ss_key 'S'
```

Defaults:

- launch mode: `pane`
- key: `S`

Launch modes:

- `popup`: open the viewer in a tmux popup
- `pane`: reuse the current pane by respawning it with `ss`
- `window`: open the viewer in a new tmux window

Optional binary override:

```tmux
set -g @ss_bin '~/.tmux/plugins/ss/bin/ss'
```

Then install/update plugins with TPM:

```text
prefix + I    install
prefix + U    update
```

`bin/ss` is a tracked wrapper that auto-builds `target/release/ss` when the
binary is missing or the git commit changed, matching the `rustasshn` model.

## Why Rust

This repo now treats the Rust tmux app as the primary runtime path.

The old Go implementation is no longer the main build/run surface because the
important requirement is tmux-aware image lifecycle management:

- pane-first launch
- pane/window focus awareness
- clearing kitty graphics when the viewer is not the active tmux pane/window

That prevents images from leaking over other tmux windows.

## Current Behavior

- each `*.md` file in the target directory is one slide
- natural filename sorting (`00_`, `01_`, `10_`, etc.)
- `![](./image.png)` style local image detection
- `[text](url)` style markdown links can be opened from the active line
- kitty/ghostty image drawing with tmux passthrough wrapping
- tmux focus polling clears images when the popup is hidden or inactive

## Keys

- `j`, `l`, `right`, `space`: next slide
- `h`, `k`, `left`, `backspace`: previous slide
- `g`, `gg`: first slide
- `G`: last slide
- `enter`: open the first markdown link on the active line, otherwise next slide
- `o`: outline / slide list
- `/`: search current slide, or filter outline when outline is open
- `n`, `N`: next / previous hit in current slide search
- `?`: help overlay
- `r`: reload slides from disk
- `:`: command prompt
- `q`: quit

Command prompt:

- `:open <path>`, `:o <path>`, or `:e <path>`: load a different slide directory or a single markdown file
- `:path` or `:p`: show the current loaded path
- `:reload` or `:r`: reload the current target
- `:quit`: quit

Live reload:

- `ss --watch <path>` or `ss -w <path>` polls the currently loaded markdown file(s) and reloads when they change

## Development

```sh
make fmt
make test
make build
```

## Examples

See `examples/` for sample slides.
