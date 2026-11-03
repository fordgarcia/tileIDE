# Extensions

An extension is a **separate program** TileIDE starts when it is needed and talks to over
stdin/stdout. It can be written in any language. TileIDE does not run VS Code extensions
(see [compatibility.md](compatibility.md)).

## Where they live
`~/.local/share/tileide/extensions/<id>/` (`$XDG_DATA_HOME`), one folder per extension,
each with an `extension.json`. Install one from the drawer's **Extensions** section
(Settings → Extensions, `Alt+Shift+X`): **From folder…** copies a folder, **Clone** runs `git clone --depth 1`.
Nothing runs while installing. There is no registry yet (planned with mods, v0.7).

## `extension.json`
```json
{
  "id": "word-count",
  "name": "Word count",
  "version": "0.1.0",
  "description": "Counts the words, lines and characters of the focused file.",
  "run": ["python3", "./main.py"],
  "activation": ["onCommand:word-count.count"],
  "commands": [{ "id": "word-count.count", "title": "Count words" }]
}
```
- `id`: a-z, 0-9, `-`, `_`, `.` (it names the folder).
- `run`: program and arguments. A program starting with `./` is inside the extension's
  folder; anything else is found on `PATH`. The process starts in the extension's folder
  with `TILEIDE_EXTENSION_DIR` set. Its stderr goes to
  `$XDG_RUNTIME_DIR/tileide-extensions/<id>.log`.
- `activation`: when it starts. Nothing starts at launch unless asked:
  - `onCommand:<id>`: one of its commands runs.
  - `onLanguage:<id>`: a file of that GtkSourceView language opens (`rust`, `python3`,
    `c`, `cpp`, `js`, `markdown`, …).
  - `onStartup` (or `*`): with TileIDE, in every workspace. The Extensions page warns
    about these: each costs memory all the time.
- `commands`: shown as buttons on the Extensions page and runnable from a key:
  `tileide.bind("ALT", "Y", "extension", "word-count.count")`.

## Per workspace
Each extension has a switch (off = never starts, anywhere) and, per workspace, a mode:
**On demand** (default: its activation events), **Always here** (starts when the workspace
is entered) or **Off here** (doesn't start from this workspace; its commands are hidden).
Choices are saved in `state.lua`. One process serves all workspaces.

## Protocol
JSON-RPC 2.0 messages framed like LSP: `Content-Length: <bytes>\r\n\r\n<json>`.

Host → extension (requests; answer each with a `result`, `null` is fine):
| Method | Params |
|---|---|
| `initialize` | `hostVersion`, `extensionDir`, `workspaceFolder` (or `null`) |
| `command/run` | `command`, `file` (path or `null`), `language` (or `null`), `text` (the focused editor's text, or `null`) |
| `shutdown` | then the notification `exit`: quit. After 2 s the process is killed. |

Extension → host (notifications):
| Method | Params | Effect |
|---|---|---|
| `window/notice` | `message` | shown on the banner |
| `editor/insert` | `text` | inserted at the focused editor's cursor |

A request from the extension is answered with "method not found" (more host methods come
later). The time from start to the `initialize` answer is the **startup** figure in the
weight meter.

## Weight meter
While the Extensions page is open, every running extension shows its startup time,
memory (resident) and CPU, refreshed every 2 s, read from `/proc`. **Suspend** freezes the
process (`SIGSTOP`: no CPU, memory kept) until **Resume**; **Stop** ends it (it starts
again on its next event). Processes an extension starts itself are not counted.

## Example
`examples/extensions/word-count/` (Python, ~50 lines): copy it with **From folder…**.
