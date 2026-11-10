# r3vu

A mouse-driven terminal IDE for TypeScript and JavaScript on macOS.

```bash
cargo install r3vu
r3vu
```

The crates.io package is `r3vu`. The command it installs is `r3vu`.

That opens the current directory. Pass a path to open another project. Hyper is the terminal it is built for.

Click to place the cursor. Type to insert. There are no modes.

## Shortcuts

| Action | Shortcut |
| --- | --- |
| Save | Ctrl+S |
| Find | Ctrl+P |
| Search in file | Ctrl+F |
| Go to definition | Ctrl+Enter or Ctrl+Click |
| Ask Claude | Option+T |
| Edit with Claude | Option+E |
| Rename symbol | Ctrl+R |
| Undo / redo | Ctrl+Z / Ctrl+Y |
| Next / previous tab | Ctrl+Tab / Ctrl+Shift+Tab |
| Git diff | Ctrl+Shift+D |
| Git panel | Ctrl+Shift+G |
| Preview Markdown | Ctrl+Shift+M |
| Toggle explorer | Ctrl+B |
| Close file | Ctrl+W |
| New file | Ctrl+N |
| Quit | Ctrl+Q |
| Shortcuts | Ctrl+/ |
| Close a popup | Esc |

Click **Find** above the explorer, or press Ctrl+P, to find a file by name and then matching text. Click **Search** there to search text across the repo. That skips `node_modules`, `.git`, and gitignored paths. Click **Help** in the status bar for the shortcut list. Click the theme name there to choose `p0`, `light`, `contrast`, `ocean`, or `ember`.

Claude is off until you turn it on. Open **Help** and click the Claude row. After that, click **Claude** in the status bar to show or hide the thread pane. Option+T anchors a conversation to the selection, or to the cursor line, and waits for Enter. Option+E asks for one edit instruction. A running turn can be stopped from that status segment or from **Stop** in the pane. Claude has to be installed and logged in; the editor does not start a turn until you ask.

Right-click a symbol for its type, Go to Definition, and Find References. Click a gutter mark to show the previous lines, and click again to hide them. Click ↩ before the line number to revert the change. A red underline is an error and a yellow one is a warning; the status bar shows that message at the cursor, and pausing the mouse shows the full text.

In a Git repository, click **Changes** on the right of the explorer border, or the branch in the status bar, to stage and commit. **Explorer** on the left of that border returns to the file tree. Click **Diff** on the right of the file border to open that same interactive diff for the current file. The file name on the left, or opening a file, returns to the buffer. `+` stages a file and `−` unstages it. Space does the same for the selected file. Commit writes only what is staged. In that diff, **Stage**, **Unstage**, and **Discard** act on one hunk. Discard asks first. Save the file before staging it.

## Config

`~/Library/Application Support/r3vu/config.toml`

```toml
explorer_width = 28
tab_width = 4
explorer_visible = true
theme = "p0"
claude_enabled = false
```

## Optional

Types and completion use the TypeScript language server. Editing continues if it is missing.

```bash
npm install -g typescript typescript-language-server
```

## Benchmarks

```bash
cargo bench --bench editor
```

That runs the Criterion suite in `benches/editor.rs` on a release build. Keystroke and reparse times are one character inserted and then deleted, so reparse includes two incremental parses. Highlight covers about 60 visible lines. These medians are from one run on this machine:

| Work | 200 lines | 5,000 lines | 30,000 lines |
| --- | ---: | ---: | ---: |
| Keystroke | 3.1 µs | 3.7 µs | 3.7 µs |
| Reparse | 512 µs | 2.15 ms | 15.1 ms |
| Viewport highlight | 526 µs | 562 µs | 614 µs |
| Open file | 426 µs | 11.0 ms | 67.8 ms |
| Search in file | 10.8 µs | 264 µs | 1.55 ms |

Loading the tree-sitter grammars took 29.5 ms. Finding a file among 20,000 paths took 1.03 ms after a 1.75 ms index. Listing a 120-file workspace took 383 µs, and searching it took 1.80 ms. Parsing `git status` took 82 µs for 500 paths and 788 µs for 5,000. Plots land in `target/criterion/`.

## Limits

- Git marks need a worktree.
- JSON, YAML, HTML, CSS, and Markdown are colored. Markdown can be previewed with Ctrl+Shift+M. Types and completion stay TypeScript and JavaScript. Other files still edit and save.
