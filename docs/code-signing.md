# Code signing for Windows and macOS: the free route (checked 2026-11-09)

Tether's releases carry self-signed certificates, both named `Tether Code Signing`: one for Windows
(`scripts/sign-windows.ps1` on the PC), one for macOS (the Mac build runner). A self-signed
certificate is trusted only where it is installed. Elsewhere the results are SmartScreen warnings
and ML detections on Windows (Defender `Bearfoos.A!ml`, Sophos `Generic ML PUA`,
docs/release-checks.md) and Gatekeeper's "Open Anyway" on the Mac.

## Status: the two certificates

| | Windows | macOS |
| --- | --- | --- |
| Public certificate | `certs/tether-codesign.cer` | `certs/tether-codesign-macos.cer` |
| SHA-1 | `9BDFE83E548CEF50DF2E96B8737ABCCD39BDC346` | `BCF53642C6C9F74356A3D12AE740BCC0EB59FB33` |
| SHA-256 | `F9:38:C0:9D:71:F9:FD:3C:E4:21:DD:31:5B:BC:B8:18:97:4E:46:E5:D5:6F:A1:D3:BE:4A:C6:CD:46:B5:CD:1D` | `17:D7:96:5C:BC:1C:11:0B:52:31:15:CE:B4:23:1B:F2:A3:46:1C:71:F8:02:B7:77:7B:3B:55:B6:42:B6:53:98` |
| Valid | 2026-11-09 to 2036-11-09 | 2026-11-04 to 2046-10-30 |
| Key (never in the repository) | `secret/tether-codesign-windows.pfx` | `C:\vms\macsign\cs.p12` |
| Signs | releases built on the PC (`scripts/sign-windows.ps1`) | every Mac release (`tools/macbuild/jobs/tether/build.sh`) |
| Trust installer | `certs/Install Tether certificate.cmd` | `certs/Trust Tether.mobileconfig` |

- `certs/` (public, safe to publish/commit): the files above and `Remove Tether certificate.cmd`.
  `scripts/trust-windows-cert.ps1` and `scripts/trust-macos-cert.sh` install only the certificate
  with the fingerprint above, whatever a server sends.
- `secret/` (gitignored, your backup — never commit): the Windows `.pfx`, its base64 and password.
  The macOS `.p12` there (`secret/tether-codesign-macos.p12`, SHA-1 `D21EA174…A4CB`) was generated
  on 2026-11-09 and is **not in use**: the releases are not signed with it. Until 2026-11-10
  `certs/Trust Tether.mobileconfig` and `certs/tether-codesign-macos.cer` held that certificate by
  mistake. A Mac that installed that profile trusts a certificate nothing is signed with: remove
  the profile ("Removing the trust" below) and install the current one.

Check a file: `openssl x509 -inform DER -in certs/tether-codesign-macos.cer -noout -fingerprint -sha256`.
The Mac build refuses a TetherQS signature from any other certificate (`build.sh`: `certificate
root = H"bcf53642…"`) and a Tether signature whose requirement differs from the installed app's.

**CI signing (optional, not used for releases).** Releases are built and signed on the PC and on
the Mac build runner, not in GitHub Actions. `desktop.yml` signs its Windows artifact when these
secrets exist (writing repo secrets is intentionally not automated; run it yourself):

```sh
gh secret set WINDOWS_SIGNING_P12       < secret/windows-p12.b64
gh secret set WINDOWS_SIGNING_PASSWORD  < secret/windows-password.txt
```

Do **not** set `MACOS_SIGNING_P12` from `secret/macos-p12.b64`: that is the unused certificate, and
an app signed with it has another designated requirement than the releases (it loses Screen
Recording/Accessibility when installed over one). Without the secret the CI Mac build is ad-hoc
signed, which the smoke test in `desktop.yml` accepts. If the secret was set from that file, delete it:
`gh secret delete MACOS_SIGNING_P12` and `gh secret delete MACOS_SIGNING_PASSWORD`.

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
`Tether Code Signing` certificate from `C:\vms\macsign\cs.p12` (SHA-1 `BCF53642…FB33`, SHA-256
`17:D7:96:5C:…:53:98`, valid until 2046), the one all installed Macs know and the one in `certs/`.
`secret/tether-codesign-macos.p12` is a **different** certificate with the same name (SHA-1
`D21EA174…A4CB`, SHA-256 `8A:AF:08:25:…:41:71`). Switching to it changes the
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
| **Windows** | Made with `scripts/make-windows-cert.ps1` → `tether-codesign.pfx` (signs the releases on the PC, and the NSIS setup in CI when the secret exists). | `scripts/trust-windows-cert.ps1` installs the `.cer` into Trusted Root + Trusted Publishers (per user, or `-Machine` for all / GPO / Intune). The signed installer is then fully trusted: no SmartScreen, no "Unknown publisher". | Mark-of-the-Web-free install on the download page (PowerShell/`curl.exe` set no MotW, so SmartScreen never checks). |
| **macOS** | The build runner's certificate (`C:\vms\macsign\cs.p12`). `scripts/make-macos-cert.sh` makes a **new** one: do not switch to it (see "Signing a release built on the PC"). | `scripts/trust-macos-cert.sh` adds the `.cer` to the keychain trusted for code signing (per user, or `--machine`). codesign then trusts the signature and TCC permissions stay stable across updates. | `scripts/install-macos.sh` (curl sets no quarantine, so Gatekeeper never prompts). |

A self-signed certificate is **not** a CA/Apple-issued identity: without the `.cer` installed, Windows
still shows "Unknown publisher" and macOS still needs one "Open Anyway" on a browser download (Gatekeeper
wants Apple notarization). The trust install closes the Windows gap completely; on macOS it stabilises the
signature, and the curl install removes the prompt. For trust on *every* machine with no per-machine step,
the paid-or-org routes at the end still apply (SignPath for Windows, Apple fee waiver for macOS).

## Make the signed apps trusted — one double-click (recommended)

`certs/` holds a ready installer per platform with the certificate built in; hand it to customers.
No command line, no store picking. `make-windows-cert.ps1` and `make-macos-cert.sh` emit such
installers for a new certificate. Checked 2026-11-10: the `.cmd` embeds `tether-codesign.cer`, and
the profile's data is `tether-codesign-macos.cer` byte for byte.

- **Windows — `Install Tether certificate.cmd`:** double-click → confirm UAC → done. It self-elevates and
  installs the cert machine-wide (Trusted Root + Trusted Publishers) with the built-in `certutil`. Then
  Tether's signed installer shows no SmartScreen and no "Unknown publisher".
- **macOS — `Trust Tether.mobileconfig`:** double-click → System Settings shows it under Profiles → click
  Install → authenticate. The native Apple way, no Terminal. It trusts the signature (stable permissions);
  a browser download still needs "Open Anyway" once unless the app is notarized, so for a no-prompt first
  run use the curl install below.

Publish all of these next to the downloads on each relay under `/data/download/` (served at
`https://<relay>/download/...`, like `install-macos.sh`; the release pruner leaves top-level files alone):
`Install Tether certificate.cmd`, `Remove Tether certificate.cmd`, `Trust Tether.mobileconfig`, the
raw `tether-codesign.cer` and `tether-codesign-macos.cer`, and the two scripts below.

**Scripted / fleet alternative** (same effect, for IT automation): `scripts/trust-windows-cert.ps1`
(`-Machine` for all users, or push the `.cer` by GPO/Intune) and `scripts/trust-macos-cert.sh`
(`--machine` for all users). Both take `https://` servers only and install nothing but the
certificate with the fingerprint above:

```
powershell -NoProfile -Command "& ([scriptblock]::Create((irm https://<relay>/download/trust-windows-cert.ps1))) -Server https://<relay>"
curl -fsSL https://<relay>/download/trust-macos-cert.sh | sh -s -- https://<relay>
```

### Removing the trust

- **Windows:** double-click `Remove Tether certificate.cmd` (confirm UAC), or
  `scripts\trust-windows-cert.ps1 -Remove -Machine` in an elevated PowerShell (without `-Machine`
  after a per-user install). By hand: `certutil -delstore Root 9BDFE83E548CEF50DF2E96B8737ABCCD39BDC346`
  and the same with `TrustedPublisher` (`certutil -user -delstore …` after a per-user install).
- **macOS, profile:** System Settings → General → Device Management (macOS 13 and 14: Privacy &
  Security → Profiles) → "Trust Tether" → − (Remove). This removes its certificate too.
- **macOS, script install:** `security delete-certificate -t -Z BCF53642C6C9F74356A3D12AE740BCC0EB59FB33`
  (after `--machine`: with `sudo` and `/Library/Keychains/System.keychain` as the last argument),
  or delete "Tether Code Signing" in Keychain Access.

### What installing the certificate means

Both installers put a self-signed certificate into the machine's trusted roots. The macOS
certificate has `basicConstraints CA:FALSE` (critical) and the code-signing usage only. The Windows
certificate in use has the code-signing usage (EKU) but **no** basicConstraints extension;
`make-windows-cert.ps1` sets `CA=false` (critical) for future certificates. The current one is not
regenerated: a new certificate has a new thumbprint, so every customer PC would have to install the
trust again. Whoever holds a private key can sign software these machines trust: keep the
`.pfx`/`.p12` offline.

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

1. **Signature:** keep the build runner's self-signed `Tether Code Signing` certificate. macOS ties
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
