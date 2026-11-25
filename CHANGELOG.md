# Changelog

All notable changes. Versions before 1.0 are betas.

## 0.11.1 (beta)

- **Fixed: the macOS app did not start at all.** `single-instance` treats its name as a *file path* on macOS;
  our bare name resolved against `/` (the working directory of an app opened from Finder), which is not
  writable, so startup failed with "file open or create error". The lock is now an absolute path in the
  temporary folder, a lock that cannot be created no longer stops the app, and a startup failure on macOS
  now shows a notification as well as writing `startup-error.txt`. Found with the new
  `.github/workflows/macos-smoke.yml`, which installs a DMG on a real macOS runner and tries to start it.
- Docs: README and website now state exactly what runs where (Windows tested, macOS beta never run on a
  Mac, Linux `aum` only) and per provider (Claude Code supported; Codex CLI experimental, off by default,
  limits only). Install guide gains a Linux section; `OWNER` placeholders replaced with the real repository.
- **Project website** in `site/`: one short page for users (what the widget shows, explained on an annotated
  picture; three steps to get started; one paragraph on privacy), published to GitHub Pages by
  `.github/workflows/pages.yml`. Needs Pages set to "GitHub Actions".
- **Update notifications (stage 1 of docs/research/auto-update.md).** New `aum-update` crate and
  Settings > Updates. Off until you say yes: the app asks once, a few seconds after the first start
  (closing the window counts as no). Once a day, or on tray menu "Check for updates…", it lists the
  project's GitHub releases through the system `curl` (https only, ETag, no credentials, generic
  user agent), remembers the result in `update.json`, and shows "Update available: X" in the tray menu plus
  one notification. Choosing it opens the release page. Nothing is downloaded or installed. Beta
  (pre-release) versions are included by default until 1.0. Needs a public repository.
- Release workflow now builds every supported platform in parallel and publishes one release: Windows
  (installer, portable zip now also containing `aum.exe`), macOS (universal DMG)
  and a Linux x64 `aum` command-line archive. Can be run by hand as a dry run that publishes nothing.
- CI: every job has a timeout and a newer push cancels the older CI run. The macOS jobs run by default;
  set the repository variable `SKIP_MACOS=true` to skip them if macOS runners are unavailable.
- CI: GitHub Actions bumped to Node 24 releases (checkout v6, upload-artifact v6, download-artifact v7,
  action-gh-release v3) to clear the Node 20 deprecation warnings.
- **Compact toggle on the widget**: a chevron button next to Refresh switches between the full and
  compact widget (same setting as Widget → Compact mode in the tray menu, so the two stay in sync).
- **Fixed: widget position after plugging in or removing a display.** Windows moves the widget by
  itself when the monitor layout changes, and the app saved those forced moves over the position you
  had chosen, so it never went back to its spot when the display returned. The app now notices layout
  changes (resolution, scaling, monitors added or removed; checked every 2 s while the widget is
  visible), ignores the system's moves, puts the widget at your saved spot if it is reachable (the
  default corner if not, without forgetting the saved spot), and re-applies opacity and rounded
  corners. Not yet verified on a real second monitor, and macOS cannot list monitors in this code
  path, so it is unchanged there.

## 0.11.0 (beta)

- **Session activity** for Claude Code: the widget, tray menu and tray icon show sessions that are
  working, waiting for you (with the reason) and recently finished, from `claude agents --json`.
  Counts only; project folders and titles are never read into the program. Unknown statuses are never
  shown as idle, and a failed read drops the numbers instead of leaving them stale.
- **WSL bridge** (Windows): optionally also read the sessions running inside named WSL distributions,
  only those already running, never starting one.
- Alerts for "needs your input" and "finished" (off by default; several at once are one alert).
- Adaptive polling: 15 s while a session is busy, 60 s when quiet, with error backoff.
- Verified on a real machine: the native Windows `claude.exe` path with default settings.
- Compact widget is a little wider to fit the activity dot; wrapped hint texts in Settings no longer
  overlap the next section.
- **Refresh interval**: besides the 1/5/15/30 minute dropdown, Settings has a box to enter any whole
  number of minutes from 1 to 1440. The dropdown switches to "Custom" for other values, and the tray
  menu's interval submenu gets a "Custom: N min…" entry that opens Settings. A hand-edited value in
  `settings.json` (for example 90 seconds) is kept exactly unless you change the interval.
- Internal: the command runner keeps raw output bytes (for UTF-16 `wsl.exe` output).

## 0.10.0 (beta)

- **Codex CLI provider** (off by default): limits read from `codex app-server` (JSON-RPC), windows
  recognised by length, extra metered buckets labelled, signed-out and API-key accounts reported
  clearly, one retry when the first answer is empty. Built against Codex's published schema; **not yet
  run against a real Codex**.
- **macOS support** (menu bar item with the session percentage, widget, settings, notifications
  through `osascript`, launch at login through a LaunchAgent, universal app bundle and DMG script,
  CI jobs). Type-checked and unit-tested for the parts that run anywhere; **not yet run on a Mac**.
- CLI lookup now also searches the usual install locations, because apps started from Finder or at
  login do not inherit the terminal `PATH`.
- Settings window: Codex section, per-provider alert switch.
- The tray icon and menu bar text follow the busiest provider.
- `aum --codex` queries Codex from the command line.
- Internal: the Windows shim became the cross-platform `platform` crate; `tray_app.rs` became `shell.rs`.

## 0.9.0 (beta)

- **Settings window** (tray menu > Settings…): provider on/off, CLI path, refresh interval, launch at
  login, widget options, alert levels and switches, test notification.
- **Notifications**: alerts at 75% and 90% (adjustable), when a limit is reached, when it resets, and
  for sign-in or read errors. One alert per limit window per level, no repeats on every poll, no repeats
  after a restart, one-minute reset-time jitter ignored, error cooldown of six hours.
- **Launch at login** through the per-user Run key.
- **Installer** (per-user, no administrator rights) and portable zip, with SHA-256 checksums.
- Timeouts now kill the whole process tree of a hung CLI (a stray child process was left behind).
- Release binary reduced from 25 MB to 13 MB.
- Documentation: install guide, troubleshooting, privacy and security, architecture, release checklist.

## 0.2.0

- **Floating widget**: progress bars coloured by level, reset countdowns, compact mode, dark and
  light themes, adjustable opacity, always-on-top, click-through, drag to move, remembered position
  with multi-monitor fallback, keyboard shortcuts (Esc hides, R or F5 refreshes).
- Rendering switched to Slint's software renderer: about 23 MB working set when idle.

## 0.1.0

- Windows tray app for Claude Code limits: session, weekly and model-specific weekly windows with reset
  countdowns, stale and error states, cached values across restarts, manual refresh, refresh interval,
  single instance, clean quit.
- Provider-capability model with explicit provenance; strict `/usage` parser that fails closed.
