# Study: code signing

Status: **research only, nothing built.** Written 2026-11-11. Facts marked *(unverified)* come from memory or
secondary sources and must be checked on the vendor's page before money or code depends on them.

**Goal.** Users can install and run the app without Windows SmartScreen or macOS Gatekeeper blocking it.
Updating is a separate study: [auto-update.md](auto-update.md) (it was split out of this document on
2026-11-25).

## Signing and updating are separate problems

Signing answers the *operating system* ("who made this binary, was it altered?"; trust root: Microsoft or
Apple). Updating answers the *app* ("is there a newer version and can it be installed safely?"; trust root: a
key we control). Do signing first: it is needed even for manual installs, and an updater must hand the OS a
binary that is still signed. See [auto-update.md](auto-update.md).

## 1. Windows signing

What the OS does: SmartScreen shows "Windows protected your PC" for an unsigned or low-reputation download.
Signing gives a named publisher; **reputation still has to be earned** by downloads. Microsoft states that
Artifact Signing, like OV certificates, gives no instant SmartScreen trust; expect a warning for the first weeks
and hundreds of clean installs ([Microsoft: MSIX signing guide](https://learn.microsoft.com/en-us/windows/msix/package/sign-msix-package-guide)).
EV certificates no longer bypass SmartScreen *(unverified; changed in 2024)*, so paying more buys little.

| Option | Cost | Fit |
|---|---|---|
| **Azure Artifact Signing** (formerly Trusted Signing) | about $10 a month *(third-party page, Oct 2026)*. Needs a paid Azure subscription and identity validation (a few business days). Organisations in US, Canada, EU and UK; **individuals only in US and Canada** | Cheapest, works from CI, no hardware token. Blocked if the publisher is an individual outside US/Canada |
| OV certificate from a CA | roughly $200 to $400 a year *(unverified)*. Private key must live on a hardware token or cloud HSM, so CI needs the CA's cloud-signing service | Works for individuals anywhere, but more setup |
| SignPath Foundation (free for open source) | free *(unverified)*; project must be public OSS with a conventional build | Worth checking once the repo is public; the signature names SignPath as the publisher |
| Microsoft Store (MSIX) | one-time registration fee *(unverified)* | The Store signs the package, no SmartScreen warning. Different packaging; tray apps and per-user data paths need testing. Later option |
| Unsigned | free | SmartScreen "unknown publisher" every time; some corporate machines block it outright |

Sources: [Microsoft: code signing options](https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/code-signing-options),
[G.D.G. Software on Artifact Signing](https://www.gdgsoft.com/faq/azure-trusted-signing-cost-effective-exe-code-signing),
[Artifact Signing getting started](https://www.itwriting.com/blog/12462-getting-started-with-azure-artifact-signing.html).

What we would sign: `agent-usage-monitor.exe`, `aum.exe` (CLI), and the Inno Setup installer **and its
uninstaller** (Inno has `SignTool` support). Always add an RFC 3161 timestamp so the signature outlives the
certificate. Nothing in the code changes; this is a CI step in `release.yml` plus secrets.

## 2. macOS signing and notarization

Requires an Apple Developer Program membership (about $99 a year *(unverified for 2026)*). Steps: sign with a
**Developer ID Application** certificate and the hardened runtime, zip or DMG it, submit with
`xcrun notarytool submit --wait`, then `xcrun stapler staple`
([Electron notarize notes](https://packages.electronjs.org/notarize) describe the same flow).
`scripts/macos-bundle.sh` already has the optional Developer ID and notarize path, never run.

- Unsigned or ad-hoc apps are quarantined by Gatekeeper. Recent macOS removed the easy right-click Open
  override for them (user must go to System Settings, Privacy and Security) *(unverified for the current OS)*.
- Apple's notary service had stuck-submission reports in 2026 (Apple developer forums), so CI needs a long
  timeout and a retry, and the result must be tested on a clean Mac.
- The hardened runtime needs no special entitlements for what we do (spawn `claude`, read its output, show a
  window), but this is untested; we have no Mac.
- CI needs: the certificate exported as a `.p12` secret, an App Store Connect API key for `notarytool`.

## 3. Proposed phases

| Phase | Work | Needs from the user |
|---|---|---|
| 1 | Decide publisher identity (individual or organisation, and country); create GitHub org/repo; replace `OWNER` placeholders | Decision |
| 2 | Windows signing in `release.yml` (Artifact Signing or OV, installer and exe, timestamped); SmartScreen reputation plan | Account, identity validation, secrets |
| 3 | macOS: Apple Developer membership, Developer ID cert, notarize in CI, clean-Mac test | Apple account, a Mac for testing |
| 4 | winget manifest, Homebrew cask, Scoop manifest (after signed releases exist; also an update channel, see auto-update.md) | Repo public, signed releases |

None of these can be verified without accounts (and, for macOS, a Mac); phase 1 is a decision, not code.

## Risks and open questions

- **Who is the publisher?** The signed name shown to users is the validated legal name. An individual's name,
  or a company, or SignPath's. This decides which signing option is even available.
- **Slint licence** (open item 6) is independent of signing but should be settled before a public release.
- SmartScreen reputation takes weeks; plan the first signed release as a beta so early warnings are expected.
- Certificate compromise: document revocation and how to rotate the signing identity.
- Unverified: SignPath terms, current Apple and Store fees, current macOS
  Gatekeeper override behaviour. Check these before choosing.

## Decisions needed

1. Publisher identity and country (individual or organisation)?
2. Windows signing route: Artifact Signing, OV certificate, or SignPath (check once the repo is public)?
3. Pay for an Apple Developer membership now, or ship macOS as unsigned beta first?
