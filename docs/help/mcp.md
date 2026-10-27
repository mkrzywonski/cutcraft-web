# Connect an AI tool to CutCraft

Start CutCraft first. Its MCP service starts with the window at
`http://127.0.0.1:47833/mcp`. Open **Edit ▸ AI Connection (MCP)…** and check it says
**Running at** that address (**Copy** copies it). The same window turns AI tools on or off,
changes the address or port, and makes access keys. Examples in this guide use your port.

Run the AI client on the same computer as CutCraft. Connecting does not launch a second
CutCraft. Most AI tools read their MCP settings only when they start: after changing the port or
a key, restart the tool (or its session).

### Access keys (optional on this computer)

With **Require an access key** ticked, only AI tools that send one of your keys can connect.
Make one key per tool under **Access keys**: type a name such as `claude`, `codex` or `grok` and
press **New key**. The key is shown **once** — press **Copy key**, or copy the ready-made setup
for Claude Code, Codex or a stdio tool from the same card. CutCraft keeps only a fingerprint of
each key. **Delete…** a key to shut that one tool out from its next request; the others keep
working. Tick **Require an access key** and **Apply** once your tools have their keys. Below,
each tool's setup is shown without a key and with one (replace `KEY` with yours).

AI tools can read these settings but never change them or make keys: only you can, in the window.

## Codex

With CutCraft running, register its HTTP endpoint:

```sh
codex mcp add cutcraft --url http://127.0.0.1:47833/mcp
codex mcp list
```

With an access key, Codex reads it from an environment variable named in its `config.toml`
(set `CUTCRAFT_MCP_KEY=KEY` in the environment Codex starts from):

```toml
[mcp_servers.cutcraft]
url = "http://127.0.0.1:47833/mcp"
bearer_token_env_var = "CUTCRAFT_MCP_KEY"
```

Alternatively, put the access key directly in your Codex `config.toml` instead of using an
environment variable. Replace `KEY` with the full key copied from CutCraft, keeping the
`Bearer ` prefix (including the space):

```toml
[mcp_servers.cutcraft]
url = "http://127.0.0.1:47833/mcp"
http_headers = { Authorization = "Bearer KEY" }
```

Use one access-key configuration for this server. **Copy Codex setup** on the new-key card
copies the direct-header version with your key already filled in. If CutCraft does not require
an access key, omit `http_headers`.

Start a new Codex session and use `/mcp` to check the connection.

Source: [official OpenAI MCP setup](https://learn.chatgpt.com/docs/extend/mcp?surface=cli).

## Claude Code

```sh
claude mcp add --transport http --scope user cutcraft http://127.0.0.1:47833/mcp
claude mcp get cutcraft
```

With an access key:

```sh
claude mcp add --transport http --scope user cutcraft http://127.0.0.1:47833/mcp \
  --header "Authorization: Bearer KEY"
```

To change the key later, `claude mcp remove cutcraft` and add it again.

Open a new Claude Code session and check `/mcp`. User scope makes this connection available
outside the current repository. Source: [Claude Code MCP setup](https://code.claude.com/docs/en/mcp).

## VS Code / GitHub Copilot

Run **MCP: Open User Configuration** and merge this entry into the `servers` object:

```json
{
  "servers": {
    "cutcraft": {
      "type": "http",
      "url": "http://127.0.0.1:47833/mcp"
    }
  }
}
```

With an access key, add `"headers": {"Authorization": "Bearer KEY"}` to the entry.

Start the server entry in VS Code, accept its trust prompt, and enable its tools in agent chat.
Source: [VS Code MCP configuration](https://code.visualstudio.com/docs/agent-customization/mcp-servers).

## Claude Desktop and other stdio clients

Use CutCraft's own stdio bridge. It connects to the running window over HTTP and forwards MCP;
it does not create a separate design state and needs no Node.js/Python proxy. In Claude Desktop,
use **Settings → Developer → Edit Config** and merge this entry into `mcpServers`:

```json
{
  "mcpServers": {
    "cutcraft": {
      "command": "/Applications/CutCraft.app/Contents/MacOS/cutcraft-cli",
      "args": ["mcp-bridge"]
    }
  }
}
```

With an access key, give the bridge the key in its environment (it is not shown in process
lists, unlike `--key KEY`):

```json
      "args": ["mcp-bridge"],
      "env": {"CUTCRAFT_MCP_KEY": "KEY"}
```

Use the absolute path to the CLI binary you actually built/installed. For the development Mac
bundle, it is inside `artifacts/CutCraft.app/Contents/MacOS/`. On Windows, use the full path to
`cutcraft-cli.exe` with forward slashes or escaped backslashes in JSON. On Linux, use the absolute
path to `cutcraft-cli`; a NixOS development build requires the repository's `nix-shell` runtime
libraries where applicable. Keep CutCraft open, then fully restart Claude Desktop.

The configuration file lives at `~/Library/Application Support/Claude/claude_desktop_config.json`
on macOS and `%APPDATA%\Claude\claude_desktop_config.json` on Windows. Other stdio clients use the
same executable/arguments in their own configuration. `mcp-bridge --port 47833` overrides discovery
of the saved port when needed. Stdout contains MCP only; startup errors go to stderr.

Source: [official local MCP client guide](https://modelcontextprotocol.io/docs/develop/connect-local-servers).

## Other AI tools (Grok and others)

Any tool that supports MCP over Streamable HTTP connects to `http://127.0.0.1:47833/mcp`; with
an access key it must send the header `Authorization: Bearer KEY`. A tool that only starts MCP
servers as commands (stdio) uses the bridge as in the previous section. Check the tool's own MCP
documentation for where these go.

## What the AI can do now

Every action is a command in one registry, shared by the window, MCP and `cutcraft-cli run`.
Each call reports the project `revision` after it; edits accept `expectRevision` and change
nothing if the project moved on meanwhile.

| Tool | Does |
|------|------|
| `command_list` | List every command with its parameters, authority and whether it is enabled now |
| `command_run` / `command_batch` | Run any command by id, or several in order; `"$N.path"` passes an earlier result on, and `atomic: true` undoes the batch if a step fails |
| `doc_inspect` | Summarise the project: source (with `status`: unchanged, changed on disk, or missing), readiness, rows, and the sheet with whether it can be planned |
| `app_status` | The current design, loading state, checks and connection status |
| `design_open` / `design_close` | Open a local SVG/SVGZ/PDF/AI/DXF, PNG, JPEG, GIF or BMP and run Make cut-ready; close it |
| `design_inspection` | The full import report: pages, fonts, every object with its id, and import warnings (AI only; the window shows Issues and Info instead) |
| `prepare_report` | The Make cut-ready result: pieces, cut lines, changes, questions with their allowed answers, blockers, and `stroked_fills` (filled shapes with an outline) |
| `prepare_decide` / `prepare_reset` | Answer one question with one of its listed choices, as its button does (also settings such as `{"decision": "outlines_on_fills", "role": "shape"}`); clear answers |
| `rows_list`, `cut_include` | The colour (or layer) rows, all cut unless unticked; ticking or unticking one |
| `sheet_set`, `sheet_place` | The sheet's material, mirror, mat, weeding box, weed lines (`weed_inverted` for a stencil: lines through the shapes) and layering marks; its position and rotation on the mat (`corner`, or `onto_grid` for the smallest move inside the cutting area and sticky grid) |
| `material_list` | Saved material presets |
| `plan_sheet` | Plan the sheet: validity, size, cut length, estimated time and problems |
| `project_open` / `project_save` | `.cutcraft` projects; a save never writes over the design |
| `export` | The Cut Preview SVG, cutter file (for inspection only) or plan, to a path or in the result |
| `render_preview` | The Cut Preview as a picture, with its scale and colours; `order` shows the cut order so far, `highlight` marks pieces, `view: tool-motion` adds blade-up travel, `view: assembly` shows every colour together, `view: source` the original design as opened; the test-cut spot (dashed) or a finished test cut's area (solid) is marked |
| `ui_screenshot` | Capture the real rendered window |
| `undo`, `redo`, `journal_list` | Undo or redo changes (each step names who made it); read what was run this session |
| `mcp_settings`, `help`, `engine_check`, `device_discover` | Connection settings (read only for AI tools: enabled, address, port, whether keys are required, key names), help guides (`topic` `ai_setup`, `designs`, `troubleshooting`, `about` or `manual`), the engine check, USB discovery |
| `mcp_port_check` | Who uses an address and port (free, this window, another CutCraft window with its process, another program, unavailable), with the first free port |
| `design_size`, `rows_group` | Resize the whole design with linked proportions (by width, height, factor, or `fit` to the piece or the mat); group rows by colour or layer |
| `sheet_adjust`, `material_save`, `material_save_current`, `material_delete` | Edit the sheet's blade depth, force, speed, passes, mirror, offset and overcut. Save current settings with `material_save_current {}`; supply a new `name` for Save As. Add, edit (`rename_from`), replace or remove library presets |
| `project_update` | *Update from design*: read the design file again and keep the sheet settings and unticked rows that still exist |
| `device_connect`, `device_disconnect`, `device_load` | Connect the real Cameo over USB, a cutter on a paired cameo-pi (`device: remote`), or the Simulated Cameo (development builds) with an optional fault scenario; press the simulator's Load / Unload button |
| `station_discover`, `station_pair`, `station_list`, `station_forget`, `station_library`, `station_open` | cameo-pi, the network cut server: find Pis on the network, ask one for access (someone allows it on the Pi's web page, using the code that comes back), list the paired Pis and their cutters, forget one, browse a Pi's job library, open a job's project from it (cutting or sending it adds a revision of that job) |
| `cut_send` | Store the prepared sheet in a cameo-pi's library without cutting |
| `device_status`, `device_wait` | The cutting state, loading steps, estimated progress and whether the sheet was cut; wait until a cut ends or pauses |
| `cut_sheet`, `cut_test`, `cut_start` | Choose the cut or a test cut (the loading steps come back), then start once the material is loaded |
| `cut_pause`, `cut_resume`, `device_stop`, `cut_done` | Pause at the end of a cut, resume, Stop now, and close a finished cut |
| `cut_confirm_fresh` | On a cameo-pi cutter whose loaded mat the Pi didn't see going in (`device_status` reports `mat_unconfirmed`): confirm it has fresh material, so Start works. Only when it's true; a mat the Pi knows was cut on is refused |
| `units_set` | Show and type lengths in inches or millimetres in the window (commands always take mm) |
| `snap_set` | Snap to grid on the Cut preview: `enabled` and/or `step_mm` (0.1–100) |
| `panel_set` | Arrange the right panel: `order` (group titles), `locked` (groups kept above the scrolling part) and/or `open` (group → open or closed) |
| `new_design_set` | The material and/or mat every newly opened design starts with, by name (a unique start is enough); `""` goes back to the usual start; `material_save` / `mat_save` also take `default` |
| `mats_list`, `mat_save`, `mat_delete` | Your mats: outer size, sticky grid, rulers per edge, where the cutter's corner falls, load length; `sheet_set` `mat_profile` picks one |
| `sheet_test_spot` | Move the test cut's 1 in square to any spot on the mat (its top-left corner in mm) clear of the artwork, or neither x nor y to place it automatically |
| `sheet_piece` | The material piece: width and height and/or x and y on the mat (from the cutter's start corner), or nothing to forget it; cuts off the piece block the cut |
| `cut_exit` | Leave the cut or test-cut screen for Cut Preview; cancel any unstarted job, retain cut records and occupied areas, refuse while cutting |
| `cut_reset` | Start over: forget that the sheet was cut or interrupted (also in the saved project) |
| `ui_inspect`, `ui_show` | What the window shows (screen, dialog, windows, focus); switch screens (`side-by-side`, `overlay`, `mat`, `cut`, `test-cut`); switching to `mat` fits cut lines that do not fit (result `adjusted` says how) |

The project, its rows, the Make cut-ready report, the materials, the sheet, each piece and
command can also be read as resources (`cutcraft://project`, `cutcraft://sheet`…). The
`prepare-design`, `diagnose-file` and `check-corpus` prompts start common workflows.

Start with: “Check CutCraft's status, open this design, and explain anything that needs review.”
Questions such as *Is this a background box?* or *Use this substitute* font belong to the person
cutting; an AI should answer one only when asked to. Answers are replayed from the original file,
and a rejected answer keeps the previous result. Use absolute paths on the computer running
CutCraft. File inspection never overwrites the source, import errors keep the previous design,
and stale results cannot replace a newer open or close.

**An AI can do anything you can in the window.** That includes cutting, test cuts, pause and stop,
so only connect AI tools you trust with the cutter. The real Cameo cuts from a mat (roll cutting waits for
its own tests); development builds can also cut on the Simulated Cameo. The cutter still needs material loaded by hand, and an AI should tell you what it is
about to cut. Everything an AI does is recorded with its origin: answers it gave are marked in the window, each
undo step names who made it, and `journal_list` lists what was run. USB discovery does not open or
claim devices, query firmware or send commands. There is no shell or raw engine-command tool.
Private designs, reports and screenshots given to an AI client become part of that client's
context; keep private outputs out of Git.

### Without the window

`cutcraft-cli mcp` serves the same tools as a headless session over stdio: no window and no USB.
It reads and writes files wherever you can; `--automation-read-root` and
`--automation-write-root` optionally limit it to folders. `--in FILE` opens a design or project
first. It never presents itself as the window: to drive the window, use `mcp-bridge`.

## Connection problems

- **Connection refused:** open CutCraft and check AI connection is running, with MCP enabled.
  Use the exact copied address, including `/mcp`. A cloud-only connector cannot reach this
  computer's loopback address.
- **Off: port in use:** another program, or another CutCraft window, already listens on the
  port, so AI tools are off in this window (another CutCraft window keeps serving them). The
  settings window says which. Close the other program and press **Try again**, or choose another
  port there and in your AI tool's settings.
- **401 / unauthorised:** keys are required and the tool sent none, an old one, or a deleted
  one. For a manually configured Authorization header, check it contains `Bearer KEY`, not
  just the key. Update the tool's configuration and restart it; make a new key if needed.
- **Changed port / MCP disabled:** update the client URL or restart the stdio bridge. Re-enable
  MCP from the window when disabled; AI tools cannot change the connection themselves.
- **Bad saved settings:** the app shows the error and leaves MCP disabled until you explicitly
  apply valid settings. Your unreadable file is preserved until then. Normal settings are stored
  in `cutcraft/settings.json` under the OS configuration directory: `~/.config` (or
  `$XDG_CONFIG_HOME`) on Linux, `~/Library/Application Support` on macOS, `%APPDATA%` on Windows.
  `CUTCRAFT_CONFIG_DIR` selects an isolated settings directory for development/tests.
- **Blank or failed capture:** the window needs a working renderer and a logged-in desktop
  session. A background SSH session on another user's macOS desktop does not establish that.
- **Different computers:** run the AI client locally with CutCraft, or explicitly forward the
  loopback port over your own SSH connection, for example:
  `ssh -L 47833:127.0.0.1:47833 user@cutcraft-host`. Keep the forwarding session open and connect
  the client to the local URL above. The service itself binds only to `127.0.0.1`.

MCP uses Streamable HTTP, supporting the versions negotiated by the pinned official Rust SDK.
The endpoint validates Host and Origin; arbitrary browser origins are rejected. Native MCP
clients normally omit Origin. Access keys are optional on this computer and required for a
network address. Tool failures are returned as MCP error results rather than successful empty reports.

## Several CutCraft windows

Each window would run its own MCP server, but only one program can listen on a port. The first
window to start takes it; in any other window AI tools are off, and its settings say the port is
in use by another CutCraft window (and which process) — servers answer `GET /cutcraft` so windows
recognise each other. Your AI tools keep working with the first window. Close it and press
**Try again** in another window to move them there.

## Listening beyond this computer

The address is 127.0.0.1 by default. One of this computer's network addresses may be chosen
instead (never 0.0.0.0, so the Host check stays meaningful); access keys are then always
required, and every request must carry `Authorization: Bearer KEY`. Anyone with a key can
control CutCraft, including cutting.

Interface text size: `font_size_set {"points": 18}` (or `command_run` with
`settings.font_size`) sets the saved menu, panel and dialog font size. The range is
10–24 points; 14 restores the default. Read `app_status` for `font_size`.

### DXF drawings

`design_open` accepts ASCII DXF model-space drawings through the same importer as the window.
Declared units are retained. A unitless drawing reports `dxf_units` under `prepare_report.pending`;
answer one of its choices, for example:

```json
{"decision": {"decision": "dxf_units", "unit": "mm"}}
```

Pass that object to `prepare_decide`; choices include `mm`, `in`, `cm`, `ft` and `m`. The
answer is kept in projects and used for the source view. Unfilled CAD paths are centreline
cuts and CAD layers are available to `rows_group`. Binary/3D input and omitted entities need
review or re-export; import warnings remain blocking. No CAD application or external process
is launched, and source files are never modified.

### Material settings

`sheet_adjust {"blade": 2, "force": 12, "speed": 4, "passes": 1,
"mirror": true, "overcut_mm": 0.2}` edits the same settings as the right panel.
Adjustments stay in this session and carry into newly opened designs. Selecting a preset
with `sheet_set {"material": "NAME"}` reloads its saved settings and discards the draft.
Saving a project still records that sheet's settings, independently of the preset library.

- `material_save_current {}` saves the current settings over the selected preset.
- `material_save_current {"name": "My vinyl"}` saves a new preset and selects it.
  The name must be unused. An optional `material` object applies editor values and saves
  atomically, but must still name the current material (use `name` to name the copy).
- `help {"topic": "troubleshooting"}` shows setting adjustments and physical checks.
- `cut_test` prepares an optional square/triangle pattern. `cut_start` explicitly confirms
  loading. After inspecting it, adjust settings yourself and try another spot if needed.

There is no tested-status tracking or material-test workspace. Old `materials.test*` and
`cut.test_result` commands are removed. Cuts never adjust or save material settings automatically.
