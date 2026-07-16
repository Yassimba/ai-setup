# herdr-jumplist

Back/forward through your pane focus history in [herdr](https://herdr.dev),
the way Alt+Left/Right (macOS: Ctrl+`-`) walks editor history in VS Code.
Every focus change is recorded; two actions jump back and forward through
that history, pruning panes that no longer exist.

## Install

```sh
herdr plugin install Yassimba/ai-setup/plugins/herdr-jumplist
```

Building requires a Rust toolchain (the build step runs `cargo build --release`).

## Keybindings

Bind the actions **directly** — no prefix. Each press is one jump, so
pressing the key three times walks back three panes. Pick chords you don't
use for text editing; `cmd`-modified keys reach herdr only in terminals that
forward them (kitty keyboard protocol — Ghostty, kitty, WezTerm), so
`ctrl+alt+left/right` is the portable choice.

```toml
# herdr config
[[keys.command]]
key = "ctrl+alt+left"        # or "cmd+[" in a cmd-forwarding terminal
type = "plugin_action"
command = "yassin.jumplist.back"
description = "focus previous pane"

[[keys.command]]
key = "ctrl+alt+right"       # or "cmd+]"
type = "plugin_action"
command = "yassin.jumplist.forward"
description = "focus next pane"
```

On Windows the action ids are `yassin.jumplist.back-win` and
`yassin.jumplist.forward-win`.

### cmd chords in terminals without the kitty protocol (e.g. Zed)

Zed's built-in terminal doesn't speak the kitty keyboard protocol, so
cmd-modified keys never reach herdr. Keep the herdr bindings on
`ctrl+alt+left/right` and let the host translate — in Zed's `keymap.json`:

```json
{
  "context": "Terminal",
  "bindings": {
    "alt-cmd-left": ["terminal::SendKeystroke", "ctrl-alt-left"],
    "alt-cmd-right": ["terminal::SendKeystroke", "ctrl-alt-right"]
  }
}
```

Pressing cmd+option+arrows then delivers the ctrl+alt chords to herdr.

Prefix bindings (`key = "prefix+..."`) also work, but herdr's prefix mode is
one-shot — you would re-press the prefix for every jump. Direct chords are
the point of this plugin.

## How it works

- A `pane.focused` event hook appends each focus change to a history stack
  in `$HERDR_PLUGIN_STATE_DIR/history.json` (capped at 50 entries,
  consecutive duplicates collapsed).
- `back`/`forward` move a cursor through that stack and focus the target via
  `herdr plugin pane focus`. Jump landings are marked so they don't record
  as new visits — forward history survives going back.
- Focusing a pane somewhere new after going back discards the forward tail,
  exactly like an editor jumplist.
- Entries whose pane can no longer be focused (closed panes) are pruned
  during the walk.
