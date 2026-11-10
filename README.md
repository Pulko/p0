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
| Toggle explorer | Ctrl+B |
| Close file | Ctrl+W |
| New file | Ctrl+N |
| Quit | Ctrl+Q |
| Shortcuts | Ctrl+/ |
| Close a popup | Esc |

Click **Find** above the explorer, or press Ctrl+P, to find a file by name and then matching text. Click **Search** there to search text across the repo. That skips `node_modules`, `.git`, and gitignored paths. Click **Help** in the status bar for the shortcut list. Click the theme name there to choose `p0`, `light`, `contrast`, `ocean`, or `ember`.

Claude is off until you turn it on. Open **Help** and click the Claude row. After that, click **Claude** in the status bar to show or hide the thread pane. Option+T anchors a conversation to the selection, or to the cursor line, and waits for Enter. Option+E asks for one edit instruction. A running turn can be stopped from that status segment or from **Stop** in the pane. Claude has to be installed and logged in; the editor does not start a turn until you ask.

Right-click a symbol for its type, Go to Definition, and Find References. Click a gutter mark to show the previous lines, and click again to hide them. Click ↩ before the line number to revert the change. A red underline is an error and a yellow one is a warning; the status bar shows that message at the cursor, and pausing the mouse shows the full text.

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

## Limits

- Git marks need a worktree.
- JSON, YAML, HTML, and CSS are colored. Types and completion stay TypeScript and JavaScript. Other files still edit and save.
