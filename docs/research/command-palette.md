# Study: showing usage in a command palette

Status: **research only, nothing built.** Written 2026-11-12. Claims about third-party products come from
their documentation and community mirrors found by search; items marked *(unverified)* need a check against the
vendor's own pages or a prototype before we commit.

**Question.** Can a user type a few letters in a command palette and see their Claude Code (and later Codex)
quota, without opening the widget?

"Command palette" is ambiguous, so this study covers the candidates and recommends one.

| Candidate | Platform | Fit |
|---|---|---|
| **PowerToys Command Palette** (`Win+Alt+Space`) | Windows | **Best fit.** Windows is our first platform, and extensions can show live list items |
| Raycast / Alfred | macOS | Easy, via scripts. Waits until macOS is proven |
| VS Code command palette / status bar | any | A separate TypeScript extension; a different product. Out of scope |
| Flow Launcher, PowerToys Run | Windows | Older launchers; not researched |

## What the user would see (Windows)

Typing "usage" opens a page with one row per enabled provider:

```
Claude Code            Session 35%  ·  Weekly 34%
                       resets in 4h 04m   ·   updated 2 min ago
Codex                  not enabled
```

- Row icon coloured by level (ok, warning, critical, unknown), same thresholds as the widget.
- Details pane: every window (session, weekly, per-model weekly) with its reset time, plus the session activity
  line ("2 working, 1 needs input") when that feature is on.
- Stale or failed data says so ("Stale: updated 40 min ago", "Not signed in"), exactly as the widget does.
- Commands on a row: copy a one-line summary; open the app (if not running, start it).
- Read-only. No account actions.

## How Command Palette extensions work

From Microsoft's [extensibility overview](https://learn.microsoft.com/en-us/windows/powertoys/command-palette/extensibility-overview)
and mirrors of its reference:

- Extensions talk to Command Palette through a **WinRT API** (`Microsoft.CommandPalette.Extensions`) and can
  be written in any language that can implement WinRT interfaces: C# is the supported path, C++/WinRT and Rust
  are possible.
- The extension is an **out-of-process COM server** registered through a packaged-app manifest
  (`windows.appExtension` for `com.microsoft.commandpalette`, with an `ExeServer` entry that starts the
  extension with `-RegisterProcessAsComServer`). Command Palette starts it on demand.
- Extensions live under `%LOCALAPPDATA%\Microsoft\PowerToys\CommandPalette\Extensions` *(per the mirrors)*, and
  PowerToys 0.100 added an extension gallery ([news summary](https://news.lavx.hu/article/powertoys-0-100-lands-with-a-rebuilt-shortcut-guide-an-extension-gallery-and-a-move-to-net-10)).
- A community Rust SDK exists: the [`cmdpal` crate](https://docs.rs/crate/cmdpal/0.3.0/source/README.md), built on
  `windows-rs`. Its maturity and licence are *(unverified)*.

**The catch: it needs a package.** The manifest is an MSIX-style package, so the extension needs package
identity, which means a signed package (or a developer-mode sideload). That ties directly to
[code-signing.md](code-signing.md): a trusted signing certificate is a prerequisite for shipping
this to anyone but ourselves.

## Where the data comes from (the real design question)

The extension must not become a second poller. Our rules: one place talks to the provider CLIs, read-only, no
credentials, no telemetry.

| Option | How | Pros | Cons |
|---|---|---|---|
| **A. Read what the tray app already wrote** | The extension reads a small status file from the app's data folder | No process spawning, no network, instant, zero extra load on `claude`. Shows the same numbers as the widget | Needs the tray app running to be fresh; otherwise shows "stale" |
| B. Run `aum --json` on demand | Extension spawns the CLI per open | Always fresh, no dependency on the tray app | Starts `claude` each time (about 1 to 3 s and a process tree), and every palette open costs a fetch; duplicates the polling logic's isolation and timeout handling |
| C. Local IPC (named pipe) to the tray app | Extension asks the app | Fresh, can trigger "Refresh now" | New attack surface and protocol to version; more code |

**Recommendation: A.** The app already persists `cache.json` for restarts. Reading that internal file would make
its layout a public contract by accident, so instead write a **separate, versioned `status.json`**:

```json
{ "schema": 1, "written_at": "2026-11-12T10:00:00Z",
  "providers": [ { "id": "claude", "name": "Claude Code", "state": "connected", "stale": false,
      "updated_at": "...", "windows": [ { "label": "Session", "percent": 35, "level": "ok",
      "resets_at": "...", "expired": false } ],
      "activity": { "working": 2, "needs_input": 1 } } ] }
```

- Counts and percentages only; the same facts the widget already shows. No paths, titles, account names, tokens.
- Written atomically (temp file and rename) whenever the model changes, at most about once a second.
- Reader rule: if `written_at` is older than a few poll intervals, the extension shows "App not running, last
  seen N min ago" rather than old numbers as current. Window `expired` handling is the same as the widget's.
- It doubles as the integration point for everything else (Raycast, a status-bar script, shell prompts), which
  is more valuable than the palette alone.

Option B can be a fallback later (`aum --json`) when the file is missing.

## macOS and others (for later)

- **Raycast Script Command** in "inline" mode shows the first output line in the list item and refreshes on an
  interval ([Raycast script commands](https://github.com/raycast/script-commands)). A 10-line script reading
  `status.json` gives "Claude 35% · 34%". Raycast extensions are TypeScript/React ([API docs](https://developers.raycast.com/llms.txt));
  not needed for this.
- **Alfred** script filters work the same way with the same file.
- Neither needs code signing from us. Both wait on macOS being verified on a real Mac.

## Build plan

| Phase | Work | Needs | Verifiable here? |
|---|---|---|---|
| 1 | `status.json` writer in the store crate (pure model to JSON, schema test, atomic write, staleness helper), documented in `docs/` | Nothing | Yes, fully |
| 2 | `aum status` CLI subcommand that prints the same summary from the file (also a useful shell-prompt feature) | Nothing | Yes |
| 3 | Raycast and Alfred example scripts in `contrib/` | Mac to test | Reading logic yes, UI no |
| 4 | PowerToys Command Palette extension (separate project in `contrib/cmdpal/`, C# thin client or Rust `cmdpal`), reading `status.json` only | PowerToys installed; Windows dev mode to sideload; signing for distribution | Partly: this PC has Windows, PowerToys unknown |
| 5 | Publish to the Command Palette gallery / winget | Signed MSIX, public repo | No |

Phases 1 and 2 stand alone and are the foundation; phase 4 is a thin presentation layer on top, which keeps the
extension free of any provider logic or process handling.

## Risks and open questions

- **Rust vs C#.** The extension is a separate packaged app. A C# extension follows Microsoft's samples and
  tooling but adds a .NET build to a Rust project; a Rust one using the community `cmdpal` crate keeps one
  toolchain but depends on a young third-party SDK *(unverified)*. Decide after a one-hour spike.
- **Extension API stability.** Command Palette is new and moving (0.100 added a gallery). Pin to a version and
  keep the extension tiny so breakage is cheap.
- **Is PowerToys installed here?** Not checked. A spike needs it.
- **Staleness is the main UX risk.** If the tray app is not running the palette must say so clearly. Starting the
  app from the palette helps but introduces a launch path to document.
- **Privacy:** the extension runs as the user and only reads `status.json`. It must not read `cache.json`,
  settings, or any provider files, and should make no network calls. State this in `docs/PRIVACY.md` when built.
- Schema discipline: `schema` is an integer; readers refuse a higher major version rather than guess.

## Decisions needed

1. Which palette did you mean: **PowerToys Command Palette** (assumed), Raycast, VS Code, or another?
2. Is a separate versioned `status.json` acceptable as a new file in the data folder (recommended), or should
   the palette read `cache.json` directly (not recommended)?
3. C# or Rust for the extension (spike first)?
4. Is shipping a signed MSIX package for this acceptable, given it depends on the signing decision?
