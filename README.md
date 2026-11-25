# twit

A lightweight terminal Git client for Linux, inspired by VS Code's *Git Graph* extension.

## Features

- **Commit graph**
- **Two-stage staging**
- **Side-by-side diff**
- **Regex search & replace**
- **Per-submodule scoping**
- **Neovim integration**
- **Embedded terminal / Neovim**
- **Live worktree watching**
- **Vim-style keyboard navigation**

## Install

Requires a Rust toolchain.

```sh
cargo install --locked --git https://github.com/s5suzuki/twit --tag v0.1.0 twit
```

Drop `--tag` to install the latest `main`.

Then:

```sh
twit <repo path>   # under Zellij or herdr, splits the current tab into panes
```

## Under Zellij or herdr

Inside a supported multiplexer (Zellij or [herdr](https://herdr.dev)), `twit` splits the current tab into panes (sidebar, changes, main, and a terminal below main) instead of one integrated window. The multiplexer is auto-detected at startup.

```sh
twit --new-tab <repo path>   # expand the split into a new tab
twit --single <repo path>    # force one integrated window even under a multiplexer
```

## Configuration

`twit` reads `$XDG_CONFIG_HOME/twit/config.toml`.

Every key is optional; anything omitted falls back to its default. The full list, with defaults:

```toml
graph_commit_limit = 200          # Max commits loaded into the graph
graph_show_author = true          # Show the author next to each commit
graph_show_email = true           # Show the author email next to each commit
graph_show_date = true            # Show the date next to each commit
graph_files_tree = true           # Expanded commit files as a folder tree (false = flat list)
confirm_discard = true            # Ask for confirmation before discarding changes
vim_input = true                  # Vim-like modal editing in the prompts; with false,
                                  # the prompts stay in insert mode and Esc cancels

[keys.global]                     # Key rebinding — see "Rebinding" below
# "ctrl+t" = "open-search"
```

Notes:

- A file that fails to parse (or contains an invalid value, e.g. `vim_input = "yes"`) is ignored and
  the defaults are used; the reason is reported on stderr at startup.
- An unknown key is ignored, and reported on stderr at startup, e.g.
  `twit: config: unknown key "vim_inptu" in /home/you/.config/twit/config.toml, ignored`.

### `init`

`twit init` writes every key above, plus every default keybinding as
`[keys.<context>]` tables, into `config.toml`.

```sh
$ twit init
twit: created /home/you/.config/twit/config.toml (7 keys, 77 bindings)
```

Keys and bindings already present keep their value, and comments and layout are preserved,
so it is safe to re-run after an upgrade to pick up newly added ones.

`twit init --force` (`-f`) ignores the current file and writes a pristine one instead, which is the
way back to the defaults after the file has been edited into a corner.
The old file is copied to `config.toml.bak` first, overwriting any earlier backup.

```sh
$ twit init --force
twit: regenerated /home/you/.config/twit/config.toml (7 keys, 77 bindings); the previous file is at /home/you/.config/twit/config.toml.bak
```

Note that a generated file pins the bindings it lists: a default that changes in a later release
no longer reaches an already-listed chord. Re-run `twit init` to append the new ones, or delete the
`[keys.*]` tables to go back to tracking the defaults.

## Keybindings

Press `?` in any pane (outside a text field or the terminal) to pop up a cheat-sheet of the bindings currently in effect.

### Defaults

**Global** (any pane)

| Key | Action | Description |
| --- | --- | --- |
| `Alt+h` | `focus-left` | Move focus to the pane on the left |
| `Alt+l` | `focus-right` | Move focus to the pane on the right |
| `Tab` | `cycle-tab-fwd` | Next right-hand tab (only while the right pane is focused; passes through to the terminal) |
| `Shift+Tab` | `cycle-tab-back` | Previous right-hand tab (only while the right pane is focused) |
| `Alt+o` | `cycle-tab-fwd` | Next right-hand tab |
| `Alt+i` | `cycle-tab-back` | Previous right-hand tab |
| `Ctrl+Tab` | `cycle-tab` | Cycle the right-hand tab (Graph → Diff → Search → Editor) |
| `Ctrl+Shift+f` | `open-search` | Open the Search tab (repository-wide search & replace) |
| `Ctrl+o` | `nav-back` | Go back in navigation history |
| `Ctrl+i` | `nav-forward` | Go forward in navigation history |

**Graph** (right pane, Graph tab)

| Key | Action | Description |
| --- | --- | --- |
| `j` | `graph-down` | Move cursor down |
| `k` | `graph-up` | Move cursor up |
| `gg` | `graph-top` | Jump to the newest commit |
| `G` | `graph-bottom` | Jump to the oldest commit |
| `Ctrl+d` | `graph-half-page-down` | Move cursor half a page down |
| `Ctrl+u` | `graph-half-page-up` | Move cursor half a page up |
| `l` / `Enter` | `graph-open` | Open the commit / file under the cursor |
| `e` | `graph-editor` | Open the file under the cursor in the editor |
| `h` | `graph-collapse` | Collapse the expanded commit |
| `o` | `graph-checkout` | Check out the commit / branch |
| `b` | `graph-create-branch` | Create a branch at the commit |
| `t` | `graph-create-tag` | Create a tag at the commit |
| `y` | `graph-cherry-pick` | Cherry-pick the commit |
| `i` | `graph-rebase-interactive` | Interactively rebase onto the commit |
| `R` | `graph-reset` | Reset the current branch to the commit |
| `V` | `graph-revert` | Revert the commit |
| `M` | `graph-merge` | Merge the commit / branch into the current branch |
| `B` | `graph-rebase-onto` | Rebase the current branch onto the commit |
| `p` | `graph-push` | Push the current branch |
| `f` | `graph-fetch` | Fetch from the remote |
|  -  | `graph-pull`  | Pull from the remote |

**Diff** (right pane, Diff tab)

| Key | Action | Description |
| --- | --- | --- |
| `/` | `diff-find` | Toggle the in-file find & replace bar |
| `j` | `diff-down` | Move cursor down one line |
| `k` | `diff-up` | Move cursor up one line |
| `gg` | `diff-top` | Jump to the first line |
| `G` | `diff-bottom` | Jump to the last line |
| `v` | `diff-toggle-visual` | Toggle visual (line) selection |
| `Esc` | `diff-clear-visual` | Clear the selection |
| `s` | `diff-stage-selection` | Stage the selected lines (when unstaged) |
| `u` | `diff-unstage-selection` | Unstage the selected lines (when staged) |
| `Ctrl+d` | `diff-half-page-down` | Scroll half a page down |
| `Ctrl+u` | `diff-half-page-up` | Scroll half a page up |
| `Ctrl+f` | `diff-page-down` | Scroll one page down |
| `Ctrl+b` | `diff-page-up` | Scroll one page up |
| `E` | `diff-editor` | Open the current file in the editor at the cursor line |

**Changes** (staging pane)

| Key | Action | Description |
| --- | --- | --- |
| `gg` | `changes-top` | Move cursor to the top |
| `G` | `changes-bottom` | Move cursor to the bottom |
| `j` | `changes-down` | Move cursor down |
| `k` | `changes-up` | Move cursor up |
| `h` | `changes-collapse` | Collapse a folder/group, or step out |
| `l` | `changes-expand` | Expand a folder/group, or open a file |
| `Enter` | `changes-activate` | Open a file, or toggle a folder/group |
| `Space` | `changes-stage-toggle` | Stage/unstage the item under the cursor |
| `e` | `changes-edit` | Open the file in the editor |
| `d` | `changes-discard` | Discard changes to the file |
| `Ctrl+d` | `changes-half-page-down` | Move cursor half a page down |
| `Ctrl+u` | `changes-half-page-up` | Move cursor half a page up |

**Sidebar** (repository tree)

| Key | Action | Description |
| --- | --- | --- |
| `gg` | `sidebar-top` | Move cursor to the top |
| `G` | `sidebar-bottom` | Move cursor to the bottom |
| `j` | `sidebar-down` | Move cursor down |
| `k` | `sidebar-up` | Move cursor up |
| `Enter` | `sidebar-select` | Select the repository under the cursor |
| `l` | `sidebar-expand` | Expand a node, or select it |
| `h` | `sidebar-collapse` | Collapse a node, or step out |
| `Ctrl+d` | `sidebar-half-page-down` | Move cursor half a page down |
| `Ctrl+u` | `sidebar-half-page-up` | Move cursor half a page up |

**Search & replace bars** (not rebindable)

Inside the Diff find bar (`/`) and the Search tab, the input fields use fixed keys:

| Key | Description |
| --- | --- |
| `Enter` | Go to the next match (Diff bar) / run the search (Search tab) |
| `Shift+Enter` | Go to the previous match (Diff bar) |
| `Esc` | Unfocus the input; press `/` again to close the Diff bar |

**Text prompts** (not rebindable)

`twit` asks for text in a prompt: commit and amend messages, branch and tag names, the diff find
query, the search query and its globs, and the graph commit limit.

Prompts open in insert mode. `Enter` confirms and `Ctrl+c` cancels in every mode.

| Key | Description |
| --- | --- |
| `←` `→` `Home` `End` | Move the cursor |
| `Backspace` `Delete` | Delete before / under the cursor |
| `Ctrl+a` `Ctrl+e` | Move to the start / end of the line |
| `Ctrl+w` `Ctrl+u` `Ctrl+k` | Delete the word before the cursor / to the start / to the end |
| `Alt+b` `Alt+f` | Move a word back / forward |
| `Esc` | Leave insert mode (`vim_input = true`), otherwise cancel the prompt |

With `vim_input = true` (the default), `Esc` switches to normal mode and a second `Esc` cancels the
prompt. Normal mode takes a count prefix and the usual single-line subset of Vim:

| Key | Description |
| --- | --- |
| `h` `l` `0` `^` `$` `\|` | Move by character, to the start / first non-blank / end, to a column |
| `w` `W` `b` `B` `e` `E` | Move by word |
| `f` `F` `t` `T` `;` `,` | Jump to a character, repeat / reverse the jump |
| `i` `a` `I` `A` | Enter insert mode before / after the cursor, at the start / end |
| `x` `X` `s` `S` `r` `~` | Delete, substitute, replace a character, toggle case |
| `d` `c` `y` + motion | Delete / change / yank over a motion (`dd` `cc` `yy` for the whole line) |
| `D` `C` `Y` | Delete / change / yank to the end of the line |
| `p` `P` | Paste after / before the cursor |
| `v` | Visual mode; then a motion and `d` `c` `y` `x` |
| `u` `Ctrl+r` | Undo / redo |

### Rebinding

Add a `[keys.<context>]` table to `config.toml` (`$XDG_CONFIG_HOME/twit/config.toml`, or `~/.config/twit/config.toml`), where `<context>` is `global`, `sidebar`, `changes`, `diff`, or `graph`.
Each entry is `"<chord>" = "<action>"`, using any action name from the tables above.

- Modifiers: `alt`, `ctrl`, `shift`, `cmd` (e.g. `"ctrl+shift+d"`).
- Two-key sequences: space-separated, e.g. `"g g"`.
- Disable a default: set the action to `"none"` (also `"unbind"` / `"disabled"`).
- Changes take effect on restart.
- `twit init` writes out the whole default keymap as a starting point (see "init" above).

```toml
[keys.diff]
"ctrl+e" = "diff-half-page-down"   # rebind a key
"d" = "none"                       # unbind a default

[keys.global]
"ctrl+t" = "open-search"
```

# Author

Shun Suzuki, 2026
