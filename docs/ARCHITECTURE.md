# Architecture

```
crates/
  core        domain models, provenance, the Provider trait, the stateless pace forecast (`forecast`)   (no I/O)
  providers   Claude adapter (runs the CLI, parses /usage) and Codex adapter
              (JSON-RPC over `codex app-server`)                 (spawns processes)
  collector   per-provider polling tasks, backoff, cache         (tokio)
  store       settings / cache / notified files, monitor placement (JSON, atomic writes)
  app         tray / menu bar + widget + settings UI             (Windows + macOS shell, tested logic; `diagnostics` builds the redacted bug-report text)
  platform    the only unsafe code, one module per OS: opacity, rounded corners, monitors,
              launch at login, notifications (Windows: Win32/registry/WinRT; macOS: AppKit/LaunchAgent/osascript)
  update      optional "is there a newer release?" check: GitHub release list via the system `curl`,
              version rules, throttling, remembered result; and, for setup-installed Windows copies, the
              verified download of the installer (signed checksum list + SHA-256, Ed25519). The pure logic is
              tested with a scripted `curl`; the shell only runs the installer after the user confirms
  cli         `aum`, a headless runner
```

## Data flow

```
claude CLI --(providers::claude)--> ProviderSnapshot --(collector)--> watch channel --> UI thread
                                                                              |-> tray (view.rs)
                                                                              |-> widget (widget.rs)
                                                                              |-> alerts (notify.rs) -> toast
```

- **One task per provider.** A provider that is slow, failing or panicking cannot affect another; each
  fetch has a hard timeout, and on timeout the CLI's whole process tree is killed.
- **Metrics carry provenance** (`Official`, `Measured`, `Estimated`). Only `Official` is produced today.
  Missing data stays missing: the UI shows "no data" or a dash, never a made-up value.
- **Stale handling.** The last good snapshot is kept through failures and flagged stale after two
  refresh intervals. A window whose reset time has passed is shown as "reset passed", not as current.
- **No continuous repaint.** The UI updates on collector events plus one 60-second tick for countdowns;
  unchanged widget data does not repaint.

## Pure vs platform code

Everything that decides *what* to show or *when* to alert (`view`, `widget`, `notify`, `form`,
`store::place_widget`) is plain Rust with unit tests that run on any OS. `shell.rs` is the thin
Windows/macOS shell around it. OS differences live in `crates/platform` (behind identical function
signatures) and in a few `cfg` blocks in `shell.rs` (native window handle, menu bar title, activation
policy, folder opener).

## Adding a provider (for example Codex CLI)

1. Implement `aum_core::Provider` in a new module under `crates/providers`, with a parser that fails
   closed (unknown output becomes an error, never a guess) and fixtures under `tests/fixtures`.
2. Add a `ProviderId` variant and its settings/notification config.
3. Register it in `Backend::start` in `shell.rs`. The tray, widget and alerts iterate over
   providers already.

## Testing

`cargo test --workspace` runs on Linux and Windows. GUI behaviour needs a real Windows session; the
README lists what has been verified and how.
