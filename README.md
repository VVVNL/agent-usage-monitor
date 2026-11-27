# Agent Usage Monitor

A small, local-first tray / menu bar app and floating widget that shows how much of your **Claude Code**
subscription limits you have used (session, weekly and any model-specific limit), when they reset, and
which Claude Code sessions are working right now, with alerts before you run out. **Codex CLI** limits are
supported experimentally.

**Status: 0.11.4 beta.** Download from [GitHub Releases](https://github.com/VVVNL/agent-usage-monitor/releases).
Website: <https://vvvnl.github.io/agent-usage-monitor/>.

### Platforms

| | Windows 10/11 x64 | macOS 11+ (Apple Silicon and Intel) | Linux x64 |
|---|---|---|---|
| **What you get** | Tray icon, floating widget, settings, alerts | Menu bar item, floating widget, settings, alerts | No app yet: the `aum` command-line tool |
| **Download** | Installer (per-user) or portable zip, both with `aum.exe` | Universal DMG | `aum` archive |
| **Tested** | Yes, on a real Windows 11 machine (Windows 10 not tried) | **Beta.** Seen running on a real Mac (2026-11-26, from a screenshot): widget with Claude Code and Codex limits. Menu bar item, notifications, launch at login and a second display are not yet confirmed | `aum` with Claude Code (in WSL Ubuntu) |

### Providers

| | Limits | Session activity | Status |
|---|---|---|---|
| **Claude Code** | Session, weekly, per-model weekly; plan and login state | Working / needs input / finished, plus the WSL bridge on Windows | Verified on Windows, and with `aum` on Linux |
| **Codex CLI** | Session, weekly, extra metered buckets | Not available (Codex has no session-listing interface) | **Beta, off by default** (Settings). Built from Codex's published schema. **Seen working against a real Codex on a Mac** (weekly limit and reset time shown); not yet compared with `/status`, and not yet tried on Windows |

See [Pending verification](#pending-verification) for exactly what must be checked before 1.0.

| | |
|---|---|
| Install | [docs/INSTALL.md](docs/INSTALL.md) |
| Something wrong? | [docs/TROUBLESHOOTING.md](docs/TROUBLESHOOTING.md) |
| Privacy and security | [docs/PRIVACY.md](docs/PRIVACY.md) |
| How it works | [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) |
| Releasing | [docs/RELEASE_CHECKLIST.md](docs/RELEASE_CHECKLIST.md) |
| Website | `site/` (published to GitHub Pages, see [Website](docs/RELEASE_CHECKLIST.md#website)) |

## How it gets data

**Claude Code.** It runs two commands that already ship with Claude Code, with no shell involved, and
parses the text:

| Command | Used for |
|---|---|
| `claude auth status` | login state, plan type, login expiry |
| `claude -p "/usage" --no-session-persistence --output-format text` | the limit lines |

**Codex CLI** (off by default; turn it on in Settings). It starts `codex app-server`, Codex's own
JSON-RPC interface over stdin/stdout, and asks it three things: the handshake (`initialize`),
who is signed in (`account/read`) and the limits (`account/rateLimits/read`). Windows are recognised
by their length (300 minutes = session, 10080 = weekly), extra metered buckets are labelled
separately, and a signed-out or API-key account is reported as such. The protocol is labelled
experimental by its authors, so Codex updates may need adapter updates. Codex reads its own login;
this app never opens `~/.codex/auth.json`.

- **Provenance:** the percentages and reset times are what the provider itself reports
  (**Official**). Nothing is estimated or invented. If a value is missing it is shown as unavailable.
- **No credentials are read.** The app never opens Claude's token files or keychain entries and has
  no network code for usage. Claude Code does the talking. The only optional request is the update
  check below.
- **Read-only.** It modifies no Claude Code files. `--no-session-persistence` stops each poll from
  writing a transcript into `~/.claude/projects/` (without it, every poll would).
- **No quota cost.** `/usage` makes no model call (zero tokens, zero cost in its own output).
- **No telemetry, no backend, no conversation content.**

## Update notifications (optional)

The app can tell you when a newer version exists. It is **off until you say yes** (asked once, a few
seconds after the first start; Settings > Updates changes it). Once a day it asks GitHub for the list of
releases through the system's `curl`; the tray menu's "Check for updates…" and Settings > Updates > "Check now"
ask on demand. When a newer version exists the menu shows "Update available: X" and you get one
notification. "Include beta versions" is on while every 0.x release is a pre-release.

- **Windows, installed with the setup program:** the entry says "install…". After you confirm, the app
  downloads the installer from the release, **verifies the project's signature on the release's checksum
  list and the installer's SHA-256**, runs it silently (no administrator rights) and restarts. Nothing is
  downloaded or run without your click; if verification fails, nothing runs. Needs the release to carry
  `SHA256SUMS.txt.sig` and the program to have the project's public key built in.
- **Portable zip and macOS:** the entry opens the release page and you install by hand. Details and exactly what is sent: [docs/PRIVACY.md](docs/PRIVACY.md#update-check-optional);
design and later stages: [docs/research/auto-update.md](docs/research/auto-update.md).
The check can only read public releases (a private fork would get "not found" and the check says so).

## Session activity

The widget and tray show whether Claude Code sessions are **working**, **waiting for you**, or
**recently finished**:

```
● needs input: permission prompt     (amber, bold)
● 2 working                          (blue)
● finished 4 min ago                 (green, fades after 10 minutes)
● 2 idle                             (grey, only when nothing else is going on)
```

- **Counts only.** The project folder, conversation title and session id that Claude Code reports are
  dropped while parsing and never stored, shown or logged.
- **Source:** `claude agents --json`, which the Claude Code docs call "the supported way to read
  session state from outside Claude Code". It is a research preview with no stability promise, so an
  unrecognised status is shown as unknown (never as idle), and if the command fails the numbers
  disappear instead of going stale ("activity unavailable").
- **Timing:** the command says what each session is doing now, not since when. "Finished" is inferred
  when a session is seen going from working to idle, so a turn shorter than the poll interval (15 s
  while something is busy, 60 s when quiet) can be missed, and sessions that were already idle when the
  app started are shown as idle, never as finished.
- **Alerts** (off by default, in Settings): when a session needs your input (with the reason), and when
  a session finishes (only if it ran at least N seconds). Several at once become one alert.
- **Windows with WSL:** sessions started by the Claude desktop app often run inside WSL, which has its
  own session store. Name your distribution in Settings (for example `Ubuntu`) and the app also runs
  `claude agents --json` inside it. Only distributions that `wsl.exe` reports as **already running**
  are asked: a stopped distribution cannot have running sessions, and the app never starts one.
- The tray icon gets a small dot (blue = working, amber = needs input) and the menu a
  "Sessions: …" line.

## Using it

A gauge icon appears in the notification area (Windows 11 may put it under the `^` arrow; drag it
out to keep it visible). Hover for the numbers; right-click for the menu.

- **Tray icon:** fill = session usage (a provider with no session limit shows its highest current limit,
  for example Codex's weekly one); green below 75%, amber 75-89%, red 90%+, dimmed when stale. With **one**
  provider enabled it is one wide gauge. With **Claude Code and Codex both enabled** it holds two narrower
  gauges side by side, **Claude Code on the left and Codex on the right**, each with its own fill, colour
  and dimming (Codex offline does not dim Claude's bar). A gauge with no current value is drawn empty. The
  activity dot sits above the bars. The tooltip and menu name each provider.
- **Floating widget:** an always-on-top card with one bar per limit, countdowns and a warning when data
  is stale or something is wrong. Drag anywhere to move it; the position is remembered and restored
  only if enough of it is still on a connected monitor. `Esc` hides it, `R` or `F5` refreshes.
  Modes: compact (one line), dark/light, opacity 30-100%, always-on-top, click-through.
  There is no taskbar button.
- **Settings…** (tray menu): provider on/off, path to the `claude` program, refresh interval
  (a dropdown of 1/5/15/30 minutes, default 5, plus a box to enter any whole number of minutes from 1 to 1440), launch at login, all widget options, and notification levels.
  Changes apply immediately.
- **Alerts** (Windows toasts or macOS notifications): at 75% and 90% used (adjustable), when a limit is reached, when it
  resets, and for sign-in or read errors. Each alert fires once per limit window, never on every
  poll and never again after a restart; error alerts have a six-hour cooldown; offline and rate-limit
  states never alert and stale data never triggers an alert.
- **Refresh now** and **Quit** are in the tray menu.

## Known limitations

- It depends on the **text format** of `/usage`. Two wordings of the reset time are understood
  (`resets Nov 10, 4:10pm` and the newer `resets Nov 26 at 12:19am`); if Anthropic changes it again, the parser shows
  "unrecognised output" or "no data" rather than guessing. Fixtures live in
  `tests/fixtures/claude/usage`.
- Percentages are whole numbers (all Claude Code prints), and reset times can wobble by a minute
  between reads; alert logic accounts for that.
- Session activity depends on a research-preview command (see above), only covers Claude Code (Codex has
  no session-listing interface), and counts only live processes: a closed terminal disappears and is
  not reported as finished. Subagents are not listed separately.
- `/usage` reflects the account, but its "what's contributing" section is a local estimate and is
  deliberately ignored.
- API-key logins have no subscription limits and show as "not available" (Claude Code and Codex).
- **Codex support has been seen on one real Codex only** (a Mac, 2026-11-26). Field names come from
  Codex's published JSON schema (`GetAccountRateLimitsResponse`), and the fixtures in `tests/fixtures/codex`
  are derived from it, not captured. The adapter retries once if the first answer has no windows (a documented quirk).
- **macOS support is beta.** See [macOS notes](#macos-notes).
- The widget window is opaque with whole-window opacity and rounded corners from Windows 11 itself
  (square on Windows 10); on macOS the corners are clipped by the window's layer and the window background
  is clear. There is no per-pixel transparency.
- Toasts are attributed to "Windows PowerShell" when running the portable build; the installer
  registers the app's own name.
- Builds are **not code-signed yet**, so Windows SmartScreen warns on first run.

## macOS notes

Written without access to a Mac, so treat the first macOS release as a beta to be tested. **0.11.0 did not
start on a Mac**: the single-instance lock used a relative file name, which macOS resolves against `/` when
the app is opened from Finder, so it failed with "file open or create error" and quit (found by running the
released DMG on a macOS runner, `.github/workflows/macos-smoke.yml`). Fixed in 0.11.1 (the lock is now an
absolute path in the temporary folder, a lock failure no longer stops the app, and a startup failure shows a
notification). 0.11.2 then fixed a second Mac-only problem (the newer `resets Nov 26 at 12:19am` wording).
A user has since run it on a real Mac (screenshot 2026-11-26: widget showing Claude Code and Codex limits),
and this version rounds the widget's corners and gives the app an icon. Still unconfirmed on a Mac: the menu bar
item's look, notifications, launch at login and a second display:

- It type-checks and passes Clippy for `aarch64-apple-darwin` and `x86_64-apple-darwin`, the text
  helpers (notification script, LaunchAgent plist) are unit-tested on every OS, the packaging
  files are validated, and CI starts the built app on a real macOS runner (`macos-smoke.yml`).
- A menu bar item (the same icon, with the percentage next to it; with Claude Code and Codex both on, one
  value each, `82% · 7%`, Claude Code first, a dash for a provider without a current value) and the same
  floating widget. The
  app runs as an accessory app: no Dock icon. The widget corners are rounded (10 pt, clipped on the content
  view's layer; the window background is clear). The app has an icon (`packaging/macos/AppIcon.icns`,
  generated by `packaging/icon/make_icons.py`).
- Notifications go through `osascript` (no signing needed); macOS attributes them to **Script Editor**
  and asks to allow them the first time. Launch at login writes
  `~/Library/LaunchAgents/io.github.agent-usage-monitor.plist`.
- Apps started from Finder do not get your terminal `PATH`, so the app also looks in
  `~/.local/bin`, `~/.claude/local`, `~/.npm-global/bin`, `~/.bun/bin`, the newest `~/.nvm` Node,
  `/opt/homebrew/bin` and `/usr/local/bin`. Set the path in Settings if yours lives elsewhere.
- The first launch of an unsigned app needs **right-click > Open**. If Claude Code's login is in the
  macOS Keychain, the first poll from the app may show a Keychain prompt.
- `scripts/macos-bundle.sh <version>` builds the universal `.app` and a DMG and signs it ad hoc;
  with a Developer ID and notary credentials in the environment it signs and notarizes it.

## Building

```bash
cargo test --workspace            # works on any OS
cargo run -p aum-cli              # headless: print current limits
cargo run -p aum-cli -- --codex  # headless: query Codex CLI instead
cargo build --release -p aum-app  # Windows or macOS: the tray app and widget
scripts/macos-bundle.sh <version> # macOS only: universal .app and DMG
```

`python3 scripts/third_party.py` regenerates [docs/THIRD_PARTY.md](docs/THIRD_PARTY.md). The installer
is built from `installer/agent-usage-monitor.iss` by CI (Inno Setup).

## Performance (measured on Windows 11)

Release build, widget showing, polling every minute (the minimum) against the real Claude Code for
15 minutes, sampled once a minute:

| Measure | Result | Target |
|---|---|---|
| Working set | 22.6 to 23.0 MB, flat | under 60 MB |
| Private memory | 4.0 to 4.3 MB, flat | |
| Handles | 237 to 249, no growth | |
| CPU over 15 minutes | 0.17 s in total (about 0.02%) | under 0.5% |
| Idle, widget showing, no polling | 0.000% CPU | |
| Stray child processes after a poll | none | none |
| Release binary | 12.9 MB | |

Cold start was not measured separately; the tray icon registers and the first data arrives within about
30 seconds, which is dominated by running `claude`. At the default 5-minute interval the app starts two
short-lived processes every five minutes and makes no network connections of its own (the optional update
check, off by default, is the one exception).

## Verification status

Run on a real Windows 11 machine with a `windows-gnu` build, driven through Win32 messages, the Win32
menu API and Windows UI Automation. The settings window and tray menu were read and operated through
the running process, not from screenshots. Screen capture of app windows is not available to this
test method, so on-screen appearance was checked from toolkit-rendered images instead.

### Tray and data (0.1)

| Check | Result |
|---|---|
| Tray icon registered, no console window, second launch exits | Verified |
| Menu content matches data; held open across minute ticks | Verified (text updates in place) |
| Refresh now, interval persisted, Quit (no ghost icon, cache saved) | Verified (repeated) |
| Offline: old values kept and flagged stale; CLI missing: actionable error | Verified |
| Icon pixels, all states, at 32 px and the 16 px downscale | Rendered and inspected |

### Widget (0.2)

| Check | Result |
|---|---|
| Placement at top-right of the work area; saved position restored; off-screen falls back | Verified |
| Always-on-top, opacity 50/70/95/100%, click-through, compact (250x193 to 210x46), hide/show | Verified (a bug where toggling always-on-top lost the opacity was found and fixed) |
| Layout in full, light, compact, stale and logged-out states | Rendered by the toolkit and inspected |

### Alerts, settings and release (0.9)

| Check | Result |
|---|---|
| 50% silent; 80, 95, 100% each alert once; repeats at the same level silent | Verified with a controllable fake CLI |
| Restart at 100% does not repeat; reset-time jitter of 1 minute does not repeat | Verified |
| New window announces the reset and re-arms; sign-in alert once, silent while it persists, re-armed after recovery | Verified |
| Toasts: 7 shown, 0 failures logged | Verified (that Windows displayed them was not observed) |
| Settings window controls operated through UI Automation: launch at login (registry value written, quoted, and removed), compact, show widget, provider on/off, alert switches | Verified |
| Hung CLI: timeout after 45 s, app stays responsive, whole process tree killed | Verified (a leaked child process was found and fixed) |
| Release binary 12.9 MB; no HTTP client in the dependency tree | Verified |
| Real mouse drag, keyboard shortcuts, second physical monitor, DPI scaling | **Not verified** |
| On-screen look of the widget and toasts | **Not verified** |

### Session activity (0.11)

| Check | Result |
|---|---|
| `claude agents --json`: structure, no transcript or session files written by polling, cost about 0.2 s and 0.14 s CPU per call | Observed on this machine |
| Parser, WSL bridge logic (UTF-16 `wsl -l`, case-insensitive names, stopped distro never started, partial failures), polling schedule, tracker, alert rules, settings validation | 80+ unit tests |
| **Real app, native Windows `claude.exe` with default settings and no shim**: live limits (Session, Weekly, Weekly (Fable)) and a sessions line | Verified (closes the "native Windows login" item) |
| **Real WSL bridge** on this machine: native only shows the Windows session; with `Ubuntu` configured it also shows the busy WSL session (the constant `sh` script survives `wsl.exe` argument handling) | Verified |
| Real app driven through none, working, needs input (with reason), working, finished, unchanged, command failure, recovery, unknown status: tray row, widget height and alerts at each step (2 alerts in total, none repeated, none invented after recovery) | Verified with a controllable fake `claude` |
| Widget layout with activity chips (full, light, compact with dot and count, idle-only) and the new Settings section | Rendered by the toolkit and inspected; two layout bugs found and fixed (clipped close button in compact mode, overlapping hint text) |
| The `waiting` status and `waitingFor` reasons from a **real** Claude Code | **Not verified**: they come from the docs; the fixtures are labelled docs-derived |
| Tray icon dot on a live taskbar; a toast actually appearing | **Not verified** (no screen access) |
| macOS: compiles and passes Clippy; started on a macOS runner by CI (`macos-smoke.yml`); a user ran it on a real Mac and saw the widget with Claude Code and Codex limits | **Partly verified** (menu bar look, notifications, launch at login, second display not yet confirmed) |

### Codex and macOS (0.10)

| Check | Result |
|---|---|
| Codex parsers and adapter against the published schema, with a scripted fake `app-server` (handshake order, signed-out, API key, RPC errors, empty-then-retry, silent server, process always closed, email never read) | 20 tests pass |
| `aum --codex` with Codex not installed | Verified: clear "not found" error, exit code 1 |
| Codex against a real `codex app-server` | **Not verified** (no Codex CLI available; install was declined for now) |
| macOS: type-check and Clippy for both Mac targets; a deliberate error proved the macOS-only code is compiled | Verified |
| macOS: notification text, LaunchAgent plist (also parsed with a strict plist parser), CLI search paths, `Info.plist` | Verified in unit tests / by parsing |
| macOS: launching, menu bar item, widget, notifications, launch at login, DMG | **Not verified** (no Mac) |
| Windows after the cross-platform refactor: widget placement/opacity, tray, Settings window with the new Codex controls, Codex switched on and off through the real window (tray gains a clear "Codex CLI not found" section, widget grows, Claude unaffected), clean Quit | Verified |

## Pending verification

These were deliberately deferred and must be done before 1.0:

- [ ] **Codex against a real Codex.** Seen working on a Mac (a screenshot). Still to do: run
      `aum --codex`, compare with `/status` in Codex, replace the schema-derived fixtures with real
      captures (redacted), and try it on Windows.
- [ ] **A real Mac, the rest.** The widget works (screenshot) and CI covers launch. Install the DMG (right-click > Open) and confirm the rounded corners and the app icon in Finder, then check: menu bar text, widget placement
      on the main and a second display, Esc/R, opacity, always-on-top, notifications, launch at login,
      `claude` found when started from Finder, Quit leaves nothing running.
- [ ] **The shipped builds.** CI built and published v0.11.0 (`windows-msvc` installer and portable zip,
      macOS DMG, Linux `aum`). Still to do: install and run the released installer and zip on Windows and
      repeat the checks above; all earlier Windows checks used a `windows-gnu` development build.
- [ ] **By eye:** tray icon on a live taskbar at 100/150/200% scaling; drag the widget with the mouse;
      `Esc` and `R`; second monitor; rounded corners and opacity; a toast actually appearing, with the
      right name after installing.
- [ ] **Slint licence.** The UI uses Slint under its royalty-free desktop licence, which requires the
      attribution shown at the bottom of Settings. Confirm that is acceptable for an MIT project (the
      alternatives are GPLv3 for the whole app, or a commercial licence).
- [ ] **Code signing** (optional, removes the SmartScreen warning).
- [ ] **Self-update on Windows, end to end.** Built and unit-tested (scripted `curl`, OpenSSL-signed
      fixtures); not yet run against a real signed release. Needs the update signing key set up
      (docs/RELEASE_CHECKLIST.md, "Update signing key"), a release that carries `SHA256SUMS.txt.sig`, and a newer
      release to update to: install the older setup, Settings > Updates > Check now > Install, and confirm the
      new version runs and `updates\` is cleaned up.
- [ ] **Update check: what the user sees.** Verified: the request, answer parser and decision against this
      project's real public release list (v0.11.0, including the ETag "unchanged" answer), the one-time
      question and its answers on Windows, and the failure path. Still to do when a release newer than the
      installed one exists: see "Update available: X" in the tray menu, the notification, and the release
      page opening (also on a Mac). `cargo test -p aum-update -- --ignored` repeats the live checks.

## Roadmap

| Version | Scope |
|---|---|
| 0.1 | Windows tray app |
| 0.2 | Floating widget |
| 0.9 | Settings, alerts, launch at login, installer, documentation |
| 0.10 | Codex CLI provider, macOS support (both unproven on real hardware/software) |
| **0.11 (this, 0.11.4)** | Session activity (working / needs input / finished) with the WSL bridge, Claude Code only; release builds for Windows, macOS and Linux (`aum`); optional update notifications; website |
| 1.0 | After the pending verification above |
| later | Code signing, Codex session activity (through Codex hooks), a Linux tray app |

## Licence

MIT, see [LICENSE](LICENSE). Not affiliated with Anthropic. The user interface is built with
[Slint](https://slint.dev) under its royalty-free licence; see [docs/THIRD_PARTY.md](docs/THIRD_PARTY.md)
for all third-party components.
