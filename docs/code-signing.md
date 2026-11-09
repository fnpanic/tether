# Code signing for Windows: options (checked 2026-11-09)

Tether's Windows files are unsigned. Machine-learning detections (Defender `Bearfoos.A!ml`, Sophos
`Generic ML PUA`, see docs/release-checks.md) and SmartScreen warnings are the result. Options, from
Microsoft's [code signing options](https://learn.microsoft.com/windows/apps/package-and-deploy/code-signing-options)
and [distribution paths](https://learn.microsoft.com/windows/apps/package-and-deploy/choose-distribution-path):

| Option | Cost | Fits Tether? |
| --- | --- | --- |
| Microsoft Store, **MSIX** submission: Microsoft re-signs the package | free (individual developer account free since 2025-09) | no, see below |
| Microsoft Store, MSI/EXE submission | the installer must already be signed with a CA certificate | no saving |
| Azure Artifact Signing (Trusted Signing) | ~$9.99/month | individuals only in the USA and Canada |
| OV certificate (Certum, Sectigo, ...), key in cloud HSM or token | Certum Open Source (cloud) from €49, standard ~€109-169/year, others $150-300/year | yes; Certum Open Source needs public source |
| SignPath Foundation | free for qualifying open-source projects | hardly: public OSI repo, CI builds, fork rules, publisher shown as "SignPath Foundation" |

## Why the free Store/MSIX path does not fit

- **Store-only.** The free signature exists only on the package the Store distributes. Downloads
  from our relays, the CDN or the download page stay unsigned; a self-built MSIX outside the Store
  needs a CA-trusted certificate like any other file.
- **Updates.** An MSIX app is installed read-only under `C:\Program Files\WindowsApps` and cannot
  replace itself. Tether's updater runs the NSIS setup (`apps/desktop/src-tauri/src/update.rs`
  `install_bytes`) and TetherQS swaps its own exe (`apps/quick/src-tauri/src/update.rs` `install`):
  neither works in an MSIX. Store builds are updated by the Store only, after certification of each
  release (hours to days), so the signed release index, rollout ramp, halt/resume, the relays'
  release-hint push and custom apps would not apply to them.
- **Installer actions.** The NSIS installer creates Windows firewall rules (hooks.nsh) and, for
  unattended access, a Windows service; MSIX can declare services only as a restricted capability
  that Store review must approve, and does not run custom install scripts.
- **TetherQS** is a portable exe a customer runs once without installing; a Store install defeats
  that, and per-customer custom-app downloads (signed profile in the download) are impossible.

A Store build would be a second product with its own update path beside the current one, and the
files on the download page would remain unsigned. The practical route is an OV certificate in
cloud signing (Certum), integrated into the release build with `signtool` and a timestamp.
