# p0

A mouse-driven terminal IDE for TypeScript and JavaScript on macOS.

```bash
cargo install p0-ide
p0
```

The crates.io package is `p0-ide`. The command it installs is `p0`.

That opens the current directory. Pass a path to open another project. Hyper is the terminal it is built for.

Click to place the cursor. Type to insert. There are no modes.

## Shortcuts

| Action | Shortcut |
| --- | --- |
| Save | Ctrl+S |
| Find | Ctrl+P |
| Search in file | Ctrl+F |
| Go to line | Ctrl+G |
| Completion | Ctrl+Space |
| Go to definition | Ctrl+Enter or Ctrl+Click |
| Find references | Ctrl+Shift+R |
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

Right-click a symbol for its type, Go to Definition, and Find References. Click a gutter mark to show the previous lines, and click again to hide them. A red underline is an error and a yellow one is a warning; the status bar shows that message at the cursor, and pausing the mouse shows the full text.

## Config

`~/Library/Application Support/p0/config.toml`

```toml
explorer_width = 28
tab_width = 4
explorer_visible = true
theme = "p0"
```

## Optional

Types and completion use the TypeScript language server. Editing continues if it is missing.

```bash
npm install -g typescript typescript-language-server
```

## Limits

- Git marks need a worktree.
- JSON, YAML, HTML, and CSS are colored. Types and completion stay TypeScript and JavaScript. Other files still edit and save.
