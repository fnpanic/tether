# Release checks: malware scanners

A manual step of every release: before publishing, the maintainer runs this on every release file
(Windows setup and TetherQS exe, Tether Manage exe, Mac `.dmg`, `.app.tar.gz` and TetherQS zip).
Nothing enforces it: neither Tether Manage nor CI looks at the result, and files have been published
before the check finished (table below).

```sh
bash scripts/release-checks.sh dist/windows/Tether_<v>_x64-setup.exe dist/quick/TetherQS-<v>.exe \
  dist/macos/Tether_<v>_universal.dmg dist/macos/Tether_<v>_universal.app.tar.gz \
  dist/macos/TetherQS-<v>-universal.zip "dist/manage/Tether Manage <v>.exe"
```

1. **Microsoft Defender** (`MpCmdRun -Scan -ScanType 3`, Windows only; skipped elsewhere).
2. **VirusTotal** (`scripts/vtscan.py`): looks the SHA-256 up, uploads unknown files (over 32 MB
   through an upload URL) and waits for the analysis, about 70 engines. The full JSON report goes
   to `tools/release/virustotal/<file>/<sha256 prefix>.json` and one line per check to
   `tools/release/virustotal/summary.md`; commit both with the release.

Verdicts: CLEAN; WARN for a single detection by an engine other than Microsoft (printed and kept in
the report, exit 0); FLAGGED for any Microsoft detection or two or more engines (exit 1); ERROR
when there is no result: the file is unknown to VirusTotal, the analysis did not finish within the
waiting time, or a request failed (exit 2). No result is never CLEAN: run the check again later.
`release-checks.sh` exits with the highest code of its checks: 0 all clean (or WARN only), 1 a
detection by Defender or VirusTotal, 2 a missing file or a check without a result. Publish only
after exit 0. A flagged file is not published until the detection is understood and, for a false positive,
reported to the vendor (Microsoft: <https://www.microsoft.com/wdsi/filesubmission>, "Software
developer", "Incorrectly detected as malware").

API key: env `VT_API_KEY` or `C:\vms\virustotal\api-key.txt`. Never commit it. The free plan allows
4 requests a minute and 500 a day: the script keeps 15.5 s between requests and asks for an
analysis' status once a minute, so a known file costs 1 request and a new one about 6-10; a full
release (6 files) stays near 60. VirusTotal's free API terms exclude business and commercial use;
a commercial release process needs a paid plan.

## Why

2026-11-09: a user behind Zscaler with Defender got "Konnte nicht heruntergeladen werden – Keine
Berechtigungen" for `Tether_0.5.30_x64-setup.exe`. Defender had flagged an earlier setup (0.5.14,
2026-11-06) as `Trojan:Win32/Bearfoos.A!ml`: a cloud machine-learning verdict, typical for unsigned
remote-access tools, which company PCs with a higher cloud block level enforce at download time
(the browser then reports missing permissions). The checks show such a verdict before users see it.
Code signing with a trusted certificate is the lasting remedy against these ML verdicts.

## False-positive reports

| Date | File | Vendor / detection | Status |
| --- | --- | --- | --- |
| 2026-11-09 | Tether_0.5.30_x64-setup.exe (`4751cc11ce30edab`) | Microsoft `Trojan:Win32/Bearfoos.A!ml` (seen on 0.5.14) | submitted (WDSI, software developer) |
| 2026-11-09 | Tether_0.5.30_universal.dmg (`0b75c85daad7d710`) | Microsoft `Trojan:Script/Wacatac.B!ml` (VirusTotal) | submitted (WDSI, software developer) |
| – | TetherQS-0.5.28.exe (`c96fb051a1e4b788`) | Trapmine `malicious.moderate.ml.score` (VirusTotal, 1/71) | not reported (minor engine) |
| – | Tether_0.5.31_x64-setup.exe (`6a9b8be88e544af6`) | Sophos `Generic ML PUA` (VirusTotal, 1/70) | not reported: Sophos offers non-customers only its Intelix portal (account), no false-positive form; published 2026-11-09 by decision |
| – | TetherQS-0.5.29.exe (`e49e7298eb002b52`) | Trapmine `malicious.moderate.ml.score` (VirusTotal, 1/71) | not reported (minor engine); published 2026-11-09 by decision |
| – | Tether_0.5.31_universal.dmg (`20346f8a565b489e`) | Microsoft `Trojan:Script/Wacatac.B!ml` (VirusTotal, 1/61), same as the 0.5.30 dmg | published before the check finished; to be reported once Microsoft answers the 0.5.30 dmg report. Update archive and TetherQS zip clean |
