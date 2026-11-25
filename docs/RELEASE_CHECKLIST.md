# Release checklist

Use for every release. Tick each box in the release PR.

## Before tagging

- [ ] All items under **Pending verification** in the README are done or consciously waived.
- [ ] `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings` and
      `cargo test --workspace` pass locally and in CI (Linux and Windows).
- [ ] Version bumped in the root `Cargo.toml` (`[workspace.package]`) and `Cargo.lock` updated.
- [ ] `CHANGELOG.md` updated; breaking or visible changes called out.
- [ ] `python3 scripts/third_party.py` re-run; any new licence reviewed. Slint attribution still shown
      in Settings.
- [ ] Fixtures still match current Claude Code output: run `claude -p "/usage"` and compare with
      `tests/fixtures/claude/usage`. Check `claude --version` against the last tested version.

## Build and test the artifacts

- [ ] Optional dry run: Actions > Release > Run workflow. It builds everything and keeps the files as
      workflow artifacts without creating a release.
- [ ] Push the tag `vX.Y.Z` (the release workflow refuses a tag that differs from `Cargo.toml`).
      One workflow builds all platforms in parallel; a single `publish` job creates the release once
      every build succeeded (and creates nothing if any build failed).
- [ ] The workflow produced: setup `.exe`, portable `.zip` (app and `aum.exe`), `SHA256SUMS.txt`;
      `aum-<version>-linux-x64.tar.gz` (command-line tool only, no Linux tray app yet) and
      `SHA256SUMS-linux.txt`; and, if `MACOS_CI=true`, the macOS files listed below.
- [ ] On a **clean Windows 10 and Windows 11** machine or VM, with Claude Code installed and signed in:
  - [ ] Installer runs without administrator rights; SmartScreen path matches INSTALL.md.
  - [ ] Tray icon, widget and tooltip show real limits within 30 seconds.
  - [ ] Second launch does not create a second icon.
  - [ ] Settings window opens; Launch at login toggles the registry value and survives a reboot.
  - [ ] Send a test notification shows a toast named *Agent Usage Monitor*.
  - [ ] Drag the widget, restart, position restored; unplug/replace a monitor, widget returns on screen.
  - [ ] 150% and 200% display scaling: icon and widget crisp, nothing clipped.
  - [ ] Sign out of Claude (`claude /logout`): sign-in alert once; sign in: recovers.
  - [ ] Offline: values marked stale, no crash.
  - [ ] Idle for 30 minutes: CPU near 0 and memory stable in Task Manager.
  - [ ] Quit leaves no process and no ghost tray icon.
  - [ ] Uninstall removes the program and the startup entry.
- [ ] Portable zip runs from a path with spaces.
- [ ] `SHA256SUMS.txt` matches the downloaded files.

## macOS (while macOS support is unproven, do all of this on a real Mac)

- [ ] The repository variable `MACOS_CI` is `true` (the macOS jobs are opt-in, see below), and the
      release workflow produced `agent-usage-monitor-<version>-macos-universal.dmg` and
      `SHA256SUMS-macos.txt`.
- [ ] On Apple Silicon **and** an Intel Mac (or Rosetta): right-click > Open works; menu bar item shows
      real limits within 30 seconds; no Dock icon; widget placed top-right below the menu bar;
      Esc/R; opacity; always-on-top; test notification; launch at login survives a reboot; Quit leaves
      no process.
- [ ] Started from Finder (not Terminal), the app still finds `claude` (and `codex`).
- [ ] If notarization credentials are configured: `spctl -a -vv` accepts the app and the DMG.

## Codex

- [ ] With a real Codex CLI signed in with ChatGPT: `aum --codex` matches `/status` in Codex, the
      fixtures in `tests/fixtures/codex` are replaced with real redacted captures, and the tests pass.

## Publish

- [ ] Edit the GitHub release notes; mark pre-release for 0.x.
- [ ] Known limitations in the README are still accurate.
- [ ] Announce where users will look; link INSTALL.md and TROUBLESHOOTING.md.

## After release

- [ ] Watch for reports of a Claude Code update changing `/usage` wording; the parser is the most
      likely thing to break.
- [ ] Open the next milestone.

## CI notes

- The macOS CI and release jobs only run when the repository variable `MACOS_CI` is `true`
  (Settings > Secrets and variables > Actions > Variables). They are off by default because a macOS job
  that never gets a runner (plan or billing limits) leaves the whole run queued and hard to cancel.
- Every job has a `timeout-minutes`, and `ci.yml` cancels an older run when a newer push arrives on the
  same branch. Neither helps a job still *waiting for a runner*, which only times out after 24 hours.
