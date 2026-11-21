# Contributing to Tether

Thank you for looking. Feedback from people who run Tether, or tried and gave up, is as welcome
as code.

## Ways to help

- **Tell us what stopped you.** If the setup, the docs or the apps confused you, open a
  [discussion](https://github.com/fnpanic/tether/discussions). That is the most useful report.
- **Report a bug** in [Issues](https://github.com/fnpanic/tether/issues). Say which app and
  version, the operating system, what you did, what you expected and what happened. A diagnostics
  bundle helps ([docs/troubleshooting.md](docs/troubleshooting.md)).
- **Report a security problem privately:**
  [Report a vulnerability](https://github.com/fnpanic/tether/security/advisories/new). Please do
  not open a public issue for it.
- **Improve the docs.** Everything under `docs/` and the README.
- **Send code.** For anything larger than a small fix, open an issue first so we agree on the
  approach before you spend time.

## Working on the code

- Build and layout: [README.md](README.md#build). The UI runs without a server:
  `cd apps/desktop && npm ci && npm run dev`.
- Before a pull request: `npx tsc --noEmit && npx vitest run` in the app you changed, and
  `cargo test` in its `src-tauri`.
- Changes to the RustDesk core go into `patches/` ([docs/upstream.md](docs/upstream.md)).
  If the change would help RustDesk too, please offer it upstream as well.
- If you change a security control, update [SECURITY.md](SECURITY.md) in the same change.
- Add a line to [CHANGELOG.md](CHANGELOG.md) under Unreleased.

## License

Tether is AGPL-3.0 ([LICENSE](LICENSE)). By contributing you agree that your contribution is
under the same license.
