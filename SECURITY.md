# Safety audit, p0 0.2.0

Local review of the editor and its locked crate graph. Advisory lookup: OSV, which includes RustSec, on 9 November 2026. The query was checked against a known-vulnerable tokio 1.0.0, which returned advisories.

p0 is safe to use on a project you trust. It is a local terminal editor. It does not listen on the network, does not download code, and does not run a shell. A remote attacker cannot connect to it. It is not a sandbox: opening a folder still runs git there, and runs the TypeScript language server if that program is on your PATH.

| Check | Result |
| --- | --- |
| Known advisories in 296 locked crates | 0 |
| `unsafe` blocks in the app | 0 |
| Outside programs started | git, and typescript-language-server when it is installed |
| Network listener or shell | No |

`cargo-audit` did not build on the review machine because of a local SDK linker error. The advisory database query used the same RustSec identifiers.

## What the app is allowed to do

| Action | How it is bounded | Result |
| --- | --- | --- |
| Edit and save files | Writes the buffer you save, through a temporary file in the same directory | Normal editor |
| Search the project | Reads text files under the opened folder, skips ignored paths, null bytes, and files over 2 MB | Read only |
| Git status and diff | Fixed git subcommands, each argument passed separately, no shell | No command injection |
| Language server | Looks up `typescript-language-server` on `PATH` and talks over stdin | Does not run a project binary |
| Copy and paste | User action. Bracketed paste is on, so pasted text is not treated as keystrokes | Clipboard only |

## Residual risk if the folder is untrusted

These are trust boundaries, not remote exploits. They matter when the folder, or the git and language-server programs on `PATH`, are not yours.

| Boundary | Where | Impact |
| --- | --- | --- |
| Git runs inside the opened folder | `src/workspace/mod.rs` | Git honors that repository's own configuration and can run a helper program it names. Opening the folder is in the same class as running git there yourself. |
| Language server is trusted | `src/language/mod.rs`, `src/app/state.rs` | A project can supply a TypeScript plugin that the language server loads. Workspace edits can name a file outside the project. Those edits stay in a buffer until you save. |
| File prompts are not jailed | `src/app/commands.rs` | New file, rename, and Save As join the typed name onto a directory. An absolute name or parent segments can leave the project. That requires the person at the keyboard. |
