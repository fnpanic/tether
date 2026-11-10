# Code signing policy

Free code signing provided by [SignPath.io](https://about.signpath.io), certificate by
[SignPath Foundation](https://signpath.org).

Status: application pending. No file has been signed by SignPath yet.

**Today (interim).** Releases are built on the maintainer's PC (Windows) and on the maintainer's
Mac build runner (macOS), not by GitHub Actions, and are signed there with the project's own
self-signed certificates named `Tether Code Signing` (fingerprints and trust files:
[docs/code-signing.md](docs/code-signing.md)). A self-signed certificate is trusted only on
machines where its public part was installed; everywhere else Windows shows "Unknown publisher".
The workflow `.github/workflows/desktop.yml` builds and tests on pushes to `main`, on pull requests
and on manual runs; it has no tag trigger and its artifacts are not the released files.

**Once SignPath has accepted the project.** Only Windows files built by this repository's GitHub
Actions workflow (`.github/workflows/desktop.yml`) from the source in this repository will be
signed with the SignPath Foundation certificate, and every signing request will be approved by
hand. Releases will then be taken from that workflow instead of the maintainer's PC, and this page
will say so.

## Team roles

| Role | Members |
| --- | --- |
| Committers and reviewers | [fnpanic](https://github.com/fnpanic) |
| Approvers | [fnpanic](https://github.com/fnpanic) |

All members use multi-factor authentication on GitHub and SignPath.

## Privacy

This program will not transfer any information to other networked systems unless specifically
requested by the user or the person installing or operating it. It talks only to the relay servers
of the organization that provides it; see [PRIVACY.md](PRIVACY.md).

## System changes

The Windows installer adds two inbound firewall rules named "Tether" (TCP 21118 and UDP 21119, local
subnet only, for direct connections) and, if unattended access is chosen, the Windows service
"Tether". The uninstaller removes both.

## Third-party components

Tether is AGPL-3.0 ([LICENSE](LICENSE)) and builds on RustDesk (AGPL-3.0) and other open-source
libraries; their sources are fetched by `scripts/bootstrap.sh` with the patches in `patches/`.
Third-party binaries that are not built here (Microsoft WebView2) are not signed by this project.
