# Installing Agent Usage Monitor

Windows 10 or 11 (64-bit) is the tested platform. A macOS build (beta, seen running on a real Mac) and a
Linux command-line tool are also published; see their sections below. No developer tools needed.

## Before you start

Agent Usage Monitor reads your limits from **Claude Code itself**, so you need:

1. **Claude Code** installed, and
2. signed in with a **claude.ai account** (Pro, Max or Team). Run `claude` once in a terminal and
   follow the sign-in prompt if you have not already. API-key logins have no subscription limits.

## Option A: installer (recommended)

1. Download `agent-usage-monitor-<version>-setup.exe` from the
   [Releases](https://github.com/VVVNL/agent-usage-monitor/releases) page.
2. Run it. It installs for your user only (no administrator prompt) into
   `%LOCALAPPDATA%\Programs\AgentUsageMonitor`.
3. Optional: tick **Start when I sign in to Windows**.
4. Finish, and the app starts.

**"Windows protected your PC" (SmartScreen).** Releases are not code-signed yet, so Windows warns on
first run. Click **More info**, then **Run anyway**. You can compare the file with `SHA256SUMS.txt`
from the release page:

```powershell
Get-FileHash .\agent-usage-monitor-<version>-setup.exe -Algorithm SHA256
```

## Option B: portable

Download `agent-usage-monitor-<version>-windows-x64-portable.zip`, unzip anywhere, and run
`agent-usage-monitor.exe`. Nothing is installed. (Toast notifications are attributed to
"Windows PowerShell" in portable mode; the installer registers the app's own name.)

## First run

- A gauge icon appears in the notification area. **Windows 11 hides new icons** under the `^`
  arrow: open it and drag the icon onto the taskbar to keep it visible.
- A small widget appears at the top-right of your main monitor. Drag it anywhere.
- Within about 30 seconds the tooltip and widget show your session and weekly limits.
- Right-click the tray icon for **Settings…**, refresh, widget options, and **Quit**.

## macOS (beta)

1. Download `agent-usage-monitor-<version>-macos-universal.dmg` from the Releases page, open it and
   drag **Agent Usage Monitor** to Applications.
2. The first launch is blocked as an unidentified developer (the app is not notarized yet): open
   **Applications**, **right-click the app > Open > Open**.
3. A gauge with the session percentage appears in the menu bar (no Dock icon). Click it for the menu.
4. Requirements: Claude Code (and/or Codex CLI) installed and signed in, macOS 11 or later.
   If the app says the CLI was not found, set its path in **Settings…**.
5. Notifications appear as coming from **Script Editor**; allow them when macOS asks.

Settings live in `~/Library/Application Support/AgentUsageMonitor/config`. Uninstall: turn off
*Launch at login*, quit, delete the app and that folder.

## Linux (command-line tool only)

There is no Linux tray app yet. The `aum` tool prints the same limits in a terminal:

```bash
tar -xzf aum-<version>-linux-x64.tar.gz
./aum-<version>-linux-x64/aum            # Claude Code
./aum-<version>-linux-x64/aum --json     # machine-readable
./aum-<version>-linux-x64/aum --codex    # Codex CLI (experimental)
```

Compare the archive with `SHA256SUMS-linux.txt` (`sha256sum -c SHA256SUMS-linux.txt`). Tested with Claude
Code in WSL Ubuntu. The same tool is included as `aum.exe` in the Windows downloads.

## Codex CLI

Codex is off by default. Install Codex and sign in with your ChatGPT account (`codex`), then switch
on **Monitor Codex CLI** in Settings. API-key logins have no subscription limits and are reported as
not available.

## Updating

Run the new installer over the old one (your settings are kept). Portable: replace the `.exe`.

## Uninstalling

- Installer: Windows **Settings > Apps > Installed apps > Agent Usage Monitor > Uninstall**.
  This also removes the sign-in startup entry.
- Portable: turn off **Launch at login** in Settings first, then delete the folder.
- Your settings and cached values are kept in `%APPDATA%\AgentUsageMonitor`. They contain no
  credentials. Delete that folder to remove them completely.

## Where things are stored

| File in `%APPDATA%\AgentUsageMonitor\config` | Contents |
|---|---|
| `settings.json` | your preferences and the widget position |
| `cache.json` | last known limits, shown (marked stale) right after a restart |
| `notified.json` | alerts already shown, so a restart never repeats them |
| `diagnostics.log` | our own status lines (alert titles, failures); never credentials or conversation text |
