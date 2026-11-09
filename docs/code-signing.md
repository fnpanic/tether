# Code signing for Windows and macOS: the free route (checked 2026-11-09)

Tether's Windows files are unsigned, and the Mac apps carry a self-signed certificate (`Tether Code
Signing`, CI secrets `MACOS_SIGNING_P12`/`MACOS_SIGNING_PASSWORD`, see docs/macos.md). Results:
SmartScreen warnings and ML detections on Windows (Defender `Bearfoos.A!ml`, Sophos `Generic ML PUA`,
docs/release-checks.md), Gatekeeper's "Open Anyway" on the Mac.

## Status: generated and in hand (2026-11-09)

Both free self-signed certificates and their double-click installers are generated for this app and
verified (keys open with their passwords; installers embed the matching cert):

- `certs/` (public, safe to publish/commit): `Install Tether certificate.cmd` (Windows),
  `Trust Tether.mobileconfig` (macOS), `tether-codesign.cer`, `tether-codesign-macos.cer`.
- `secret/` (gitignored, your backup — never commit): the `.pfx`/`.p12`, their base64, and passwords.

Both certs are codesign/signtool-verified: the Windows cert signs a binary locally, and the macOS
cert was verified on the build Mac on 2026-11-09 (job via `tools/macbuild`): `codesign -s "Tether Code
Signing"` signs, and the designated requirement names the certificate (the criterion `smoke-macos.sh`
checks) — `designated => identifier ... and certificate leaf = H"d21ea174..."`.

**One step left — activate CI signing** (writing repo secrets is intentionally not automated; run it
yourself, e.g. prefix with `!` in Claude Code). Setting the macOS ones replaces the prior self-signed
cert; the next macOS CI run's smoke test verifies codesign accepts the new one:

```sh
gh secret set WINDOWS_SIGNING_P12       < secret/windows-p12.b64
gh secret set WINDOWS_SIGNING_PASSWORD  < secret/windows-password.txt
gh secret set MACOS_SIGNING_P12         < secret/macos-p12.b64
gh secret set MACOS_SIGNING_PASSWORD    < secret/macos-password.txt
```

Then publish `certs/*` to each relay under `/data/download/` so customers can fetch the installers.

## Signing a release built on the PC

Windows release builds are made on the PC (docs/updates.md, "Publishing a release"). Sign them
**before** Tether Manage publishes, because Manage hashes and minisigns the files as they are:

```powershell
powershell -File scripts/sign-windows.ps1 dist\quick\TetherQS-<v>.exe dist\windows\Tether_<v>_x64-setup.exe
```

It uses `secret/tether-codesign-windows.pfx` and `secret/windows-password.txt` (SHA-256, DigiCert
timestamp). Status `UnknownError` only means this PC does not trust the self-signed root yet.

macOS: the build runner (`tools/macbuild/jobs/tether/build.sh`) signs both apps with the
`Tether Code Signing` certificate from `C:\vms\macsign\cs.p12` (SHA-256 `17:D7:96:5C:…:53:98`,
valid until 2046), the one all installed Macs know. `secret/tether-codesign-macos.p12` is a
**different** certificate with the same name (`8A:AF:08:25:…:41:71`). Switching to it changes the
designated requirement: every Mac would lose Screen Recording/Accessibility at the next update, and
`build.sh` refuses the build (requirement new ≠ old). Keep the runner's certificate.

## Custom TetherQS apps

A custom app ([quick-support.md](quick-support.md#custom-apps-branded-downloads)) is the signed
release file plus an admin-signed profile, added by the relay at download time. The code signature
must survive that:

- **Windows.** Bytes appended after an Authenticode signature break it: Windows reports the file as
  `NotSigned`. `tether-api` therefore puts the profile **inside** the certificate table
  (`tether_trust::custom::attach_plan`): zero padding to 8 bytes, then the usual trailer
  (`<JSON><u32 len>TQSPROF1`, still the last bytes of the file). It also raises the table size in
  the PE header's security directory and in the `WIN_CERTIFICATE` entry. The Authenticode hash
  leaves the table out, so the signature stays `Valid`. TetherQS reads the trailer from the file's
  end as before. Its self-update (0.5.30+) attaches the profile the same way. An unsigned exe
  gets the trailer appended, as before.
  Limit: Windows' opt-in `EnableCertPaddingCheck` (MS13-098 hardening, off by default) rejects
  data after the PKCS#7 blob, so on such a PC the custom exe counts as unsigned (it still runs).
- **macOS.** The relay only renames the bundle folder inside the zip
  (`<Name>-Support [<mac id>].app`). The folder name is not sealed, so `codesign --verify --deep
  --strict` passes and the designated requirement is the plain TetherQS one.

Checked on 2026-11-09:
- PC: `dist/quick/TetherQS-0.5.29.exe`, signed with the key, then the profile attached:
  `Get-AuthenticodeSignature` reports `Valid` (signer `CN=Tether Code Signing`), and
  `signtool verify /pa` succeeds. The same file with the profile appended the old way reports `NotSigned`.
- Windows end to end (`apps/desktop/e2e/manage-e2e.mjs`, 22/22): TetherQS built with the test keys
  and signed with `scripts/sign-windows.ps1`, published by Tether Manage to two local relays running
  the new `tether-api`. A custom app was created there and downloaded through `/q/<token>`; the
  download is `Valid` like the plain exe and runs as the branded app (name, logo, welcome text,
  custom setting). Then new link and revoke.
  Run: `WEBVIEW2_USER_DATA_FOLDER=C:\vms\signtest\wv2 MANAGE_EXE=… QS_EXE=<signed exe>
  MAC_ZIP_OUT=<zip> node e2e/manage-e2e.mjs`.
- macOS (build Mac, `tools/macbuild` job): the `/q/<token>/mac` zip from the same run,
  `ACME Support-Support [962374fbb9a64586].app`: `codesign --verify --deep --strict` "valid on
  disk, satisfies its Designated Requirement", the same requirement and signer as the plain
  `TetherQS-0.5.29-universal.zip` (`identifier "net.tether.quick" and certificate root =
  H"bcf53642…"`), and TetherQS recognises its mac id. `spctl` rejects both (self-signed, not
  notarized: the curl install or "Open Anyway" as above).

## Chosen approach: self-signed certificate + easy manual trust install

Generate one free self-signed certificate per platform, sign the builds with it, and let each user (or
their IT) install the **public** `.cer` once to trust it. No cost, no application, no public repo.

| | Certificate (free, in hand) | Make it trusted on a PC/Mac | Fallback with no cert install |
| --- | --- | --- | --- |
| **Windows** | `scripts/make-windows-cert.ps1` → `tether-codesign.pfx` (signs the NSIS setup in CI). | `scripts/trust-windows-cert.ps1` installs the `.cer` into Trusted Root + Trusted Publishers (per user, or `-Machine` for all / GPO / Intune). The signed installer is then fully trusted: no SmartScreen, no "Unknown publisher". | Mark-of-the-Web-free install on the download page (PowerShell/`curl.exe` set no MotW, so SmartScreen never checks). |
| **macOS** | `scripts/make-macos-cert.sh` → `tether-codesign.p12` (the `MACOS_SIGNING_P12` secret; already signs the apps). | `scripts/trust-macos-cert.sh` adds the `.cer` to the keychain trusted for code signing (per user, or `--machine`). codesign then trusts the signature and TCC permissions stay stable across updates. | `scripts/install-macos.sh` (curl sets no quarantine, so Gatekeeper never prompts). |

A self-signed certificate is **not** a CA/Apple-issued identity: without the `.cer` installed, Windows
still shows "Unknown publisher" and macOS still needs one "Open Anyway" on a browser download (Gatekeeper
wants Apple notarization). The trust install closes the Windows gap completely; on macOS it stabilises the
signature, and the curl install removes the prompt. For trust on *every* machine with no per-machine step,
the paid-or-org routes at the end still apply (SignPath for Windows, Apple fee waiver for macOS).

## Make the signed apps trusted — one double-click (recommended)

`make-windows-cert.ps1` and `make-macos-cert.sh` each also emit a ready installer with the certificate
built in; hand it to customers. No command line, no store picking (verified 2026-11-09: the embedded
certificate round-trips to the real one; the profile's data decodes to the cert).

- **Windows — `Install Tether certificate.cmd`:** double-click → confirm UAC → done. It self-elevates and
  installs the cert machine-wide (Trusted Root + Trusted Publishers) with the built-in `certutil`. Then
  Tether's signed installer shows no SmartScreen and no "Unknown publisher".
- **macOS — `Trust Tether.mobileconfig`:** double-click → System Settings shows it under Profiles → click
  Install → authenticate. The native Apple way, no Terminal. It trusts the signature (stable permissions);
  a browser download still needs "Open Anyway" once unless the app is notarized, so for a no-prompt first
  run use the curl install below.

Publish all of these next to the downloads on each relay under `/data/download/` (served at
`https://<relay>/download/...`, like `install-macos.sh`; the release pruner leaves top-level files alone):
`Install Tether certificate.cmd`, `Trust Tether.mobileconfig`, and the raw `tether-codesign.cer`.

**Scripted / fleet alternative** (same effect, for IT automation): `scripts/trust-windows-cert.ps1`
(`-Machine` for all users, or push the `.cer` by GPO/Intune) and `scripts/trust-macos-cert.sh`
(`--machine` for all users).

## Windows, free route 1: self-signed certificate (in hand now)

`scripts/make-windows-cert.ps1` generates a real code-signing certificate for free (verified
2026-11-09: it signs with a timestamp; the embedded signer is `CN=Tether Code Signing`). Like the
macOS self-signed certificate, it is not trusted by default on a random PC, but:

- **Managed fleets (full trust, free):** a customer's IT deploys the public `.cer` to Trusted Root +
  Trusted Publishers via GPO or Intune; the apps are then fully trusted there — no SmartScreen, no
  "Unknown publisher" — at no cost. This is the usual path for a B2B remote-support tool.
- **Stable publisher identity** for every build, instead of none.

```powershell
pwsh scripts/make-windows-cert.ps1 -OutDir .\secret -Password <pfx-password>
# keep tether-codesign.pfx + password secret (never commit); the .cer is public, for customers' IT
```

CI signs the NSIS setup with it when the secrets `WINDOWS_SIGNING_P12` (base64 of the .pfx) and
`WINDOWS_SIGNING_PASSWORD` exist (`desktop.yml`, step "Sign (self-signed)"); inert without them.
On an unmanaged PC this adds no trust by itself — pair it with the Mark-of-the-Web-free install
below, or use route 2 for universal trust.

## Windows, free route 2: SignPath Foundation (CA-trusted)

[Conditions](https://signpath.org/terms.html), checked against Tether:

| Condition | Tether today | To do |
| --- | --- | --- |
| OSI license for all components, no commercial dual licensing, no proprietary parts (system libraries allowed) | AGPL-3.0 (RustDesk, rustdesk-server), Apache (webrtc-sctp); the installer bundles only `tether.exe` and Microsoft's WebView2 bootstrapper | add a `LICENSE` file at the repo root |
| Source repository the team owns; binaries built from source verifiably | `fnpanic/tether` is **private**; Windows builds already run in GitHub Actions (`desktop.yml`, `tauri build --bundles nsis`) | make the repo public (AGPL requires offering the source to users anyway); sign from the CI artifact via SignPath's GitHub action |
| Released already, functionality documented on the download page | yes (relay download page) | |
| "Code signing policy" section on home and download page: "Free code signing provided by SignPath.io, certificate by SignPath Foundation", team roles, privacy policy or no-transfer statement | missing | add to README and the download page |
| Privacy: data transfers need a privacy policy shown at install with an opt-out where unspecified | audit log, metrics and update checks go to your own relays | write a short privacy policy, link it in the installer |
| System changes only with proper warnings; matching uninstaller | firewall rules + service (hooks.nsh); NSIS uninstaller exists | name the firewall rules and the service in the installer text |
| No hacking tools | remote support with visible consent/session UI | describe the consent flow in the application |
| MFA for all team members on SignPath and GitHub; manual approval of every release | | enable; approve each signing request |

**Prepared in the repo (2026-11-09):** `LICENSE` (AGPL-3.0), `CODE_SIGNING_POLICY.md` (attribution,
roles, privacy, system changes, third-party components), `PRIVACY.md`, the README section, and the CI
step in `desktop.yml` that submits the NSIS setup to SignPath (`signpath/github-action-submit-signing-request@v3`)
and uploads `tether-<target>-signed`. The step stays inert until the secret `SIGNPATH_API_TOKEN` exists.

**Steps left (yours, outward-facing):**

1. Make `fnpanic/tether` public (check history for secrets first: `git log -p | grep -i -E 'secret|password|BEGIN .*PRIVATE'`).
2. Enable MFA on GitHub; apply at [signpath.org/apply](https://signpath.org/) with the repo URL, the
   download page and `CODE_SIGNING_POLICY.md`.
3. After acceptance: install the SignPath GitHub App, create project `tether` with signing policy
   `release-signing`, add the secret `SIGNPATH_API_TOKEN` and the repo variable `SIGNPATH_ORGANIZATION_ID`.
4. Release the CI-signed setup (`tether-x86_64-pc-windows-msvc-signed`) instead of the local build,
   change the download page footer (`apps/manage/src-tauri/src/release.rs` `page_render`,
   `docs/examples/download-template.html`) to the attribution, and drop "(application pending)".
   TetherQS needs the same: a CI job building it, signed by the same step.

Precedents using the
same policy wording: [Pandoc](https://pandoc.org/code-signing-policy.html),
[Ink/Stitch](https://inkstitch.org/code-signing-policy), [Drawpile](https://drawpile.net/codesigningpolicy/),
[OpenRCT2](https://openrct2.io/code-signing-policy). Risk: SignPath may reject at its discretion (no
remote-desktop project was found among its signed projects); the fallback is an OV certificate
(Certum Open Source, cloud key, from about €49/year). Signed binaries also fix Smart App Control,
which blocks unsigned apps however they were downloaded.

### Until then: install without the Mark of the Web

SmartScreen reacts only to files carrying the Mark of the Web (`Zone.Identifier` stream), which
browsers add. Checked on this Windows 11 PC on 2026-11-09:

```
Invoke-WebRequest ... -OutFile iwr.bin   → Zone.Identifier: False
curl.exe -o curl.bin ...                 → Zone.Identifier: False
```

The relay download page offers this per Windows app as a second option **next to the Download button**
(`apps/manage/src-tauri/src/release.rs` `download_page`): a "Copy install command (no security prompt)"
button. The browser builds the line from the page's own origin and the release's path, so it is correct
on whichever relay serves the page:

```powershell
powershell -NoProfile -Command "$o=Join-Path $env:TEMP 'tether-setup.exe'; iwr '<relay>/download/.../Tether_x64-setup.exe' -OutFile $o; & $o"
```

`iwr` (Invoke-WebRequest) writes no Mark of the Web, so SmartScreen does not warn; the normal Download
link stays as the fallback and for browsers with JavaScript off. UAC still says "Unknown publisher"
(unsigned), and Smart App Control, when on, still blocks. Defender's ML detections are unaffected; only
a signature and reputation help there.

## macOS: no free certificate, free Gatekeeper-free install

Gatekeeper assesses an app only when the file carries `com.apple.quarantine`, which browsers set and
`curl` does not (Apple DTS: "most Unix-y tools don't quarantine their downloads, including curl";
[forum thread](https://developer.apple.com/forums/thread/706273), [who decides to quarantine
files](https://eclecticlight.co/2025/12/08/who-decides-to-quarantine-files/)). So:

1. **Signature:** keep the self-signed `Tether Code Signing` certificate (already in CI). macOS ties
   Screen Recording/Accessibility to its designated requirement, so permissions survive updates.
2. **First install:** `scripts/install-macos.sh`, published on every relay as
   `/data/download/install-macos.sh` (served at `https://<relay>/download/install-macos.sh`; the
   release pruner only touches `<channel>/<app>/<platform>/` folders):
   ```sh
   curl -fsSL https://<relay>/download/install-macos.sh | sh -s -- https://<relay>              # Tether
   curl -fsSL https://<relay>/download/install-macos.sh | sh -s -- https://<relay> customer     # TetherQS
   curl -fsSL https://<relay>/download/install-macos.sh | sh -s -- 'https://<relay>/q/<token>/mac'  # custom app
   ```
   It reads `latest.json`, downloads over HTTPS only, checks the bundle id and
   `codesign --verify --deep --strict`, installs into /Applications (or ~/Applications), and opens it.
   No "Open Anyway", no admin password.
3. **Updates:** the apps download updates themselves (no quarantine), so no prompt after the first install.

Limits: the script must be trusted like the relay (HTTPS); Apple fixes Gatekeeper bypasses, but the
curl behaviour is documented, long-standing, and how Homebrew's installer and rustup work.
A customer has to paste one Terminal line, about as much effort as "Open Anyway" with its password.
**Research conclusion (2026-11-09, searched to exhaustion):** there is no free Gatekeeper-trusted
certificate for an individual, and no foundation or third-party service notarizes open-source macOS
apps for free (notarization needs your own Developer ID certificate, which comes only with the paid
program; [Apple forum 121113](https://developer.apple.com/forums/thread/121113),
[GoReleaser](https://goreleaser.com/customization/sign/notarize/), [Tauri notarization](https://www.electron.build/docs/features/code-signing/notarization/)).
SignPath signs Windows only, not Apple.

The one fee-free trusted route is an organization: a nonprofit (e.g. a German e.V., which needs seven
founders and charitable status from the tax office), accredited school, or government gets the Apple
Developer Program by [fee waiver](https://developer.apple.com/support/fee-waiver), with a D-U-N-S number
(free), free apps only, no in-app purchases. That yields a real Developer ID + notarization at no cost.
For a bare individual, the free route is the self-signed certificate + curl install above (works, no
cost, not Apple-trusted). To remove every warning on a browser download without an org, $99/year for Developer ID + notarization is the only way;
CI already accepts a Developer ID identity (`APPLE_SIGNING_IDENTITY` secret), notarization is not wired yet.

## Microsoft Store / MSIX: free signature, but it does not fit

Microsoft re-signs MSIX packages submitted to the Store for free (developer accounts are free), but:

- **Store-only.** The free signature exists only on the package the Store distributes. Downloads
  from our relays, the CDN or the download page stay unsigned; a self-built MSIX outside the Store
  needs a CA-trusted certificate like any other file.
- **Updates.** An MSIX app is installed read-only under `C:\Program Files\WindowsApps` and cannot
  replace itself. Tether's updater runs the NSIS setup (`apps/desktop/src-tauri/src/update.rs`
  `install_bytes`) and TetherQS swaps its own exe (`apps/quick/src-tauri/src/update.rs` `install`):
  neither works in an MSIX. Store builds are updated by the Store only, after certification of each
  release (hours to days), so the signed release index, rollout ramp, halt/resume, the relays'
  release-hint push and custom apps would not apply to them. MSIX's own off-Store updater (an
  `.appinstaller` file on our server, polled by App Installer) still needs a CA-trusted signature
  and would replace our updater, so it saves no certificate.
- **Installer actions.** The NSIS installer creates Windows firewall rules (hooks.nsh) and, for
  unattended access, a Windows service; MSIX can declare services only as a restricted capability
  that Store review must approve, and does not run custom install scripts.
- **TetherQS** is a portable exe a customer runs once without installing; a Store install defeats
  that, and per-customer custom-app downloads (signed profile in the download) are impossible.

## Other options considered

| Option | Cost | Note |
| --- | --- | --- |
| Azure Artifact Signing (Trusted Signing) | ~$9.99/month | individuals only in the USA and Canada; organizations also EU/UK |
| OV certificate, key in cloud HSM | Certum Open Source from ~€49/year, others $150-300/year | fallback if SignPath declines |
| EV certificate | $400+/year | no SmartScreen advantage over OV since 2024 |
| Sigstore | free | not trusted for Authenticode or Gatekeeper |
| Self-signed / own CA | free | trusted only where the certificate is installed (managed fleets via Intune/GPO or MDM) |
