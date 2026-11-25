# Study: auto-update without an update server

Status: **stage 1 (notify only) is built** (0.11.x, see CHANGELOG and docs/PRIVACY.md); stages 2 and 3 are
design only. Decisions taken 2026-11-25: stage 1 go, automatic check off until enabled and asked once, channel
default `beta`, transport `curl`. Split out of the former signing-and-updates study on
2026-11-25; code signing is in [code-signing.md](code-signing.md). Items marked *(unverified)* come from
memory or secondary sources and must be checked before code depends on them.

**Goal.** Users learn about, and where safe install, new versions without anyone running an update server,
while keeping the project rules: local-first, read-only, no telemetry, nothing surprising on the network.

**Constraint from the request.** No update server. The only thing we operate is the GitHub repository, and
`release.yml` already publishes everything an updater needs: installer, portable zip, DMG, Linux archive and
`SHA256SUMS*.txt`, on the GitHub Releases page (v0.11.0 is the first real release).

## Review of the earlier recommendation

The earlier study recommended "notify only first, one-click later" and listed Velopack, Sparkle and Tauri's
updater as framework options. After checking the repository and the release setup:

| Earlier idea | Verdict |
|---|---|
| Notify-only first | **Keep.** It is the smallest safe step and ships value alone |
| Velopack, Sparkle, Tauri updater | **Drop.** Each replaces our packaging (Inno Setup, the macOS bundle script) or brings a manifest and hosting model of its own. We already have working installers; a second packaging system is more cost than the update logic itself |
| "Check GitHub Releases API" | **Needs two corrections**, below |
| Add an HTTP client crate | **Reconsider**: `docs/PRIVACY.md` currently promises "there is no HTTP client in the program"; use the OS's `curl` instead (see Transport) |
| Dual signature before any install | **Keep, but stage it**: only the one-click path executes downloaded code |

Two facts found while reviewing that change the design:

1. **`/releases/latest` will find nothing.** GitHub's "latest release" endpoint returns the newest
   non-prerelease, non-draft release *(from general knowledge; unverified here, check the REST docs)*, and
   our workflow marks every `v0.*` tag as a prerelease. Until 1.0, discovery must list releases and choose
   the highest version itself.
2. **Unauthenticated API calls are limited to 60 per hour per IP address**
   ([GitHub rate limits](https://docs.github.com/en/rest/using-the-rest-api/rate-limits-for-the-rest-api)).
   That is plenty for one check a day, but offices behind one NAT can exhaust it. Whether a `304 Not Modified`
   answer is free for unauthenticated callers is not clearly documented
   ([best practices](https://docs.github.com/en/rest/using-the-rest-api/best-practices-for-using-the-rest-api)),
   so handle a refused call quietly and never rely on it.

## Design in one picture

```
 GitHub Releases (static files, already produced by release.yml)
        ^                    ^
        | list releases      | download asset (only after consent)
        |  (ETag, 1/day)     |
   +----+--------------------+------------------+
   |  aum-update crate: pure logic              |   <- everything below the line is unit-tested
   |  version compare, release JSON parser,     |
   |  decision engine, checksum + signature     |
   +----+--------------------+------------------+
        | Fetcher trait (fake in tests)
   +----+--------------------+
   |  curl via the process runner (https only, size and time limits, host allowlist)
   +---------------------------+
```

No server of ours anywhere. GitHub is the distribution channel, and the trust root is a **key we hold** (stage 2),
not GitHub's hosting.

## Transport: shell out to `curl`

| Option | Pros | Cons |
|---|---|---|
| **`curl` as a subprocess** (recommended) | No new dependency, no TLS stack linked into our binary, the privacy statement stays "no HTTP client in the program", uses the OS trust store. Windows 10 1803+ ships `curl.exe` in System32 and macOS ships `/usr/bin/curl` *(unverified for every edition)*. We already run `claude` the same way, with timeouts and process-tree kill | Depends on curl existing (when absent the feature reports "update check unavailable"); no certificate pinning |
| `ureq` plus rustls | Self-contained | Adds a TLS stack and a few hundred KB to MB to a 13 MB binary *(unmeasured)*; breaks the "no HTTP client" promise |

Fixed arguments, never built from release data: `--proto =https --tlsv1.2 --fail --silent --show-error
--max-time <n> --max-filesize <n>`, a generic `User-Agent` without version or machine data, and a host
allowlist checked before the call (`api.github.com`, `github.com`, and the asset CDN host GitHub redirects to,
which must be confirmed from a real download *(unverified)*). Reuse the existing runner so timeouts and
process-tree cleanup behave like the provider calls.

## Discovery

`GET https://api.github.com/repos/VVVNL/agent-usage-monitor/releases?per_page=10` with `If-None-Match` using
the stored ETag.

- Skip drafts. Take the **highest semantic version**, not the newest by date (a hotfix to an old line must not
  look newer).
- Channel setting: `stable` ignores prereleases, `beta` includes them. **Default `beta` until 1.0**, otherwise
  nobody ever sees an update.
- Parse only: `tag_name`, `prerelease`, `draft`, `html_url`, `published_at` and, per asset, `name`,
  `browser_download_url`, `size`. Fail closed on anything unexpected, like the Claude parsers; keep a fixture
  recorded from the real v0.11.0 release.
- The repository is a compile-time constant (forks change one line). The `OWNER` placeholders (since replaced) in
  `docs/INSTALL.md` and `installer/agent-usage-monitor.iss` get replaced at the same time.
- Alternative if the API ever becomes a problem: the public `releases.atom` feed (no rate limit, includes
  prereleases, XML). Not needed now.

## Consent model (privacy)

- **Off by default.** A Settings checkbox "Check GitHub for new versions once a day", unchecked. The project
  promises no surprise network use; a first-run prompt is the alternative if you prefer discoverability.
- **"Check for updates now"** in the tray menu always works, even with the automatic check off. A click is
  the consent for one request.
- The automatic check runs at most once per 24 h, a few minutes after start with random jitter, never blocks
  the quota tasks, and failures are silent (a line in `diagnostics.log`).
- What GitHub can see: the user's IP address and a generic client name. Nothing from Claude or Codex.
- `docs/PRIVACY.md` must change when this ships: today it says "no update check" and "no HTTP client". The
  new text: the app itself makes one kind of request, to GitHub, only if you enable it or click the menu item.

## Stages

### Stage 1: notify only (all platforms, no code execution)

Tray item "Update available: 0.12.0" (and a line in Settings), opening the release page in the browser. Nothing
is downloaded or run by the app. The "Check now" result also covers "up to date" and "could not check".
Needs no signing key and no new secrets. **This alone is the recommended first deliverable.**

### Stage 2: one-click install on Windows (installed copies only)

Only for the installer-based install; portable zip users stay on stage 1 (detect by the executable's folder
being the Inno install dir, or the uninstall registry key).

1. User confirms: "Install 0.12.0 now? Agent Usage Monitor will restart."
2. Download `agent-usage-monitor-<v>-setup.exe` and `SHA256SUMS.txt` (+ signature) to a private temp folder
   with `--max-filesize` and https only.
3. **Verify all of these:** SHA-256 matches the line in `SHA256SUMS.txt`; the detached signature of
   `SHA256SUMS.txt` verifies against the **public key compiled into the app** (minisign or ed25519; a tiny
   verify-only crate such as `minisign-verify` *(unverified; check size and licence)*); and, once Windows
   signing exists, the installer's Authenticode publisher equals the expected name (via `WinVerifyTrust` in
   the platform crate, or PowerShell's `Get-AuthenticodeSignature`).
4. Refuse if the version is not strictly greater than the running one, or the file name or size is unexpected.
5. Start the installer detached with `/SILENT /CLOSEAPPLICATIONS /RESTARTAPPLICATIONS /NORESTART`
   ([Inno command-line parameters](https://jrsoftware.org/ishelp/topic_setupcmdline.htm)), then quit.
   The installer is per-user (`PrivilegesRequired=lowest`), so **no UAC prompt**. Note the installer script
   currently sets `RestartApplications=no`; the restart must be made to work and be tested.
6. The new version starts; the downloaded installer is kept for a few days for manual rollback.

Why the offline signature on top of the checksum: `SHA256SUMS.txt` lives in the same release as the file it
protects, so a hijacked release changes both. The signature binds the checksums to **a key that is not on
GitHub**. Where to keep that key:

| Key custody | Protects against | Cost |
|---|---|---|
| CI secret, signed in `release.yml` | Tampering after the build | A stolen GitHub token or CI compromise still signs malware |
| **Offline, signed by hand per release** (recommended for stage 2) | Account, CI and release tampering | One manual step: download `SHA256SUMS.txt`, sign, upload the `.minisig` to the release |

Stage 2 is the only part that executes downloaded code, so it is the only part worth that manual step.

### Stage 3: macOS and Linux

- **macOS:** notify only, with the Homebrew hint if a cask exists. In-app replacement of the `.app` is
  possible later (verify `codesign --verify --deep --strict` and the signature above, replace the bundle,
  relaunch) but needs write access to `/Applications`, a Developer ID signature to be clean, and a real Mac
  to test; nothing here can be verified without one. Not planned until macOS is verified.
- **Linux CLI:** notify only, or nothing: users manage it themselves.

### Package managers as update channels

winget, Scoop and Homebrew update installs through their own tooling and their own review. They need no code
in our app, only manifests once signed releases exist (see code-signing.md, phase 4). A package-managed
install should preferably say "update available, run `winget upgrade`" instead of offering stage 2; detecting
that reliably is *(unverified)*.

## Implementation outline

```
crates/update/         new, pure logic, no GUI
  version.rs           semver compare (small hand-written parser, or the `semver` crate)
  release.rs           parse the releases JSON (fail closed), pick the best candidate per channel
  decide.rs            Idle | UpToDate | Available{version, url} | Unavailable(reason); throttle (24 h, jitter)
  verify.rs            sha256 line lookup, detached signature check (stage 2)
  fetch.rs             trait Fetcher; CurlFetcher over the existing runner; FakeFetcher for tests
store                  settings: update_check (bool, default false), update_channel (stable|beta),
                       last_update_check, etag (in cache, not settings)
app                    tray item + Settings checkbox + "Check now"; stage 2 confirm dialog
scripts / release.yml  (stage 2) documented signing step; nothing else changes
```

Tests (all without a network): version ordering including prereleases and `0.9` vs `0.10`; the release picker
on the recorded fixture and on drafts, prereleases, hotfix-to-old-line and malformed JSON; throttling and
jitter with an injected clock; ETag round trip with the fake fetcher; checksum and signature success and every
failure; the downgrade refusal; the host allowlist. One manual end-to-end against the real repository.

## Phases

| Phase | Work | Needs | Verifiable here |
|---|---|---|---|
| 1 | `update` crate: versions, parser with the real v0.11.0 fixture, decision logic, fake fetcher, tests | Nothing | Yes |
| 2 | `CurlFetcher`, settings, "Check now" and optional daily check, tray item opening the release page, PRIVACY.md and README text | Nothing | Yes on this Windows machine (curl.exe, real GitHub) |
| 3 | Generate the update key pair offline; add the documented signing step to the release checklist | You, once | Partly |
| 4 | Windows one-click install: download, verify, launch Inno, fix `RestartApplications`, restart test | Phase 3; a published release newer than the installed one | Yes, with two real releases |
| 5 | Authenticode check of the installer | Windows signing in place | Needs signing |
| 6 | macOS in-app replace | A Mac and Developer ID | No |

Phases 1 and 2 are independent of signing and deliver the whole stage 1 feature.

## Risks and open questions

- **A bad release reaches everyone who enabled checks.** There is no kill switch without a server. Mitigations:
  drafts and deleted releases are ignored, the dry-run workflow exists, and stage 2 needs a user click.
- **Key loss** (stage 2): shipping a new key needs a release signed with the old one; document the recovery
  (a manual download) in the checklist.
- **Rate limit behind shared IPs:** handled by silent failure and backoff, not by retry loops.
- **curl absent or old:** the feature degrades to "unavailable"; verify the flags on the oldest supported
  Windows 10 build.
- **Inno `/RESTARTAPPLICATIONS` with `CloseApplications=force`** needs a real test; the app must not hold the
  installer open by waiting on it.
- **Forks and local builds** would check the upstream repository unless the constant is changed; document it.
- Unverified: `releases/latest` prerelease behaviour (design avoids it), 304 handling for unauthenticated
  calls, the asset CDN host, `minisign-verify` size and licence, curl presence on every supported Windows.

## Decisions needed

1. **Stage 1 now** (notify only, off by default, manual "Check now"): go ahead?
2. **Default for the automatic check:** off until the user enables it (recommended), or ask once at first run?
3. **Stage 2:** do you want one-click install at all, accepting a manual offline signing step per release?
4. **Channel default** before 1.0: `beta` (recommended; otherwise no update is ever offered) or `stable`?
5. **Transport:** `curl` subprocess (recommended) or a linked HTTP client crate?
