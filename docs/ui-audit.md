# UI layout audit

Every window of the three apps checked for content that is cut off, half visible, overflowing,
overlapping or unreachable; rules in [DESIGN.md](../DESIGN.md). Last run 2026-11-08.

Run it again: `node tools/ui-audit.mjs <desktop|quick|manage> <label> [scenario-regex]` (after
`npm ci` in the app). It starts the app's Vite server, drives it in Edge (mock mode; TetherQS gets a
stand-in for the Tauri IPC), takes a screenshot of each state and checks each one in the page:
elements clipped by a container that does not scroll, elements outside the window, text cut off
without an ellipsis, interactive elements covered by another element of the same layer, and a
page that scrolls sideways. Since round 2 also: a table or control that only fits by scrolling
sideways inside its box (`hscroll`), a scroll area squeezed below 60 px (`scroll-tiny`), text cut
with "…" and no tooltip (`ellipsis-notitle`) and a placeholder wider than its empty field
(`placeholder-cut`); since round 3 a control whose label wraps taller than the control itself
(`label-spill`: the extra line spills out of the button). Results: `findings.json` next to the screenshots. `UIAUDIT_OUT` sets the
output folder, `CLONE` the checkout to audit, `UIAUDIT_PORT` the Vite port.

## What was checked

Each state in light and dark, at every size of its window:

| Window | Sizes (CSS px) | States |
| --- | --- | --- |
| Tether main, Simple | 1000×660 (default), 820×560 (min), 1280×720, 1093×614 (1366×768 at 125 %), 911×512 (at 150 %) | Home, many recent devices, More menu, long ID input, Settings (all 7 sections), Recordings |
| Tether main, Advanced | 1280×800 (default), 1000×600 (min), 1280×720, 1093×614, 911×512 | 5,000 hosts, no hosts, no match, detail panel, selection bar, palette, tag rules, tag colours, Team, open sessions, keyboard help, Settings |
| Tether connection window | 380×560 (fixed), 380×512 | two clients, second tab |
| Tether session | 1280×800 (default), 640×400 (min), 1280×720, 1093×614, 911×512 | toolbar, remote actions menu, lock confirmation, quality menu, statistics, chat; file transfer |
| TetherQS | 420×560 (fixed), 420×512 | idle, click-only, long branding and logo, update and macOS banners, update required, request (long name), request with banners, session, chat with long messages, two sessions, two sessions with chat, Help/Diagnostics, recording consent |
| Tether Manage | 1180×800 (default), 900×600 (min), 1280×720, 1093×614, 911×512 | unlock, all 10 pages, 10,000 devices, 4,500 session events, new group, add technician, new custom app, diagnostics ticket, policy publish sheet |

Window chrome that only a real window has (reasoned from code): the Windows title bar controls
(120 px, drawn by the page only in Tauri) still fit the Advanced title bar at 911 px; Manage uses
native decorations; Tether and TetherQS draw their own title bar (decorations off).

## Fixed

| Where | Size | What was wrong | Fix |
| --- | --- | --- | --- |
| TetherQS Help | all | Diagnostics rows below the window edge, no scrolling | view scrolls |
| TetherQS idle with update and macOS banners | 420×512 | status footer pushed off the bottom | column scrolls |
| TetherQS, Tether windows | screen shorter than the window (1366×768 at 150 %) | window centred taller than the screen: own title bar with Close above the screen | shrunk to the work area and re-centred before it is shown |
| Tether connection window | short screen | bottom-right placement put the window above the screen's top | never above the top edge |
| Advanced host list | 1000×600 and smaller, detail panel open | Tags, OS, Version, Last active columns cut off at the panel | columns that do not fit are dropped (list width, not window width) |
| Advanced detail panel | 1000×600 and smaller | Name field and Notes squashed to slivers | panel scrolls, fields keep their height |
| Advanced selection bar | 1000×600 and smaller | Copy IDs, Remove, Clear under the detail panel | bar wraps |
| Advanced status bar | 911×512 | counts wrapped onto a second, clipped line | one line; search timing only from 1280 px |
| Advanced keyboard help | 911×512, 1093×614 | Done below the window edge | lists scroll; key caps no longer wrap |
| Session toolbar | 640×400 | toolbar wider than the window, both ends cut off | name and quality label give way under 800 px |
| Session chat | 640×400 | chat panel covered the toolbar | panel stays below the toolbar |
| File transfer | 640×400 | long host name wrapped in the pane header | truncates (full name in the tooltip) |
| Manage Devices, Sessions | 911×512 | IDs wrapped mid-number, group picker squeezed to its arrow | IDs on one line, picker keeps its width; the table scrolls sideways inside its card |

## Round 2 (2026-11-08)

A blind review found what round 1 let through: the tool accepted anything inside a sideways
scroller, missed a scroll area of 0 px, and five scenarios never reached their state (mock gaps
and disabled buttons: desktop `home-search-none`, `session-keyboard`; Manage `gate-error`,
`diag-open`, `releases-publish`). The mocks now cover them (`?hosts=N` fills Recent, a session
keyboard menu, the passphrase "wrong" fails), the scenarios click enabled buttons, and the four
checks above were added. "Before" is the new tool on f134d56 with only the test mocks updated.

| Where | Size | What was wrong | Fix |
| --- | --- | --- | --- |
| Manage Releases: What devices get | 1093×614, 911×512, 900×600 | 927 px table in an 839 / 646 px box: "Publish to miss…" cut | platform and channel under the app; "Missing" under the relay's column; "Publish to missing" (full text in the tooltip) |
| Manage Releases: Rollouts, release list | 900×600, 911×512 | Halt and Everyone now cut; list 660 px | app with platform below; buttons stacked |
| Manage Devices | 900×600, 911×512 | Group column and picker cut (791 px) | version · OS under the profile, IP under last seen: five columns |
| Manage Sessions | 900×600, 911×512 | 718-786 px table | IP under the host |
| Manage Status and relays | 900×600, 911×512 | 648 px table | table cells pad 12 px instead of 16 (every table) |
| TetherQS request with update and macOS banners | 420×512, 420×560 | permission list 0 px / 41 px: switches not visible before Accept | the card keeps its height (the list at least three rows), the window scrolls; Accept stays reachable |
| Tether session keyboard menu | 640×400 | Force Quit item below the window | menu scrolls |
| TetherQS, Tether (connection window, Recent, Advanced, file transfer) | 420 px, 911×512 | names cut with "…" and no tooltip | tooltip with the full name |
| Advanced search | 911×512 | hint cut | shorter hint, full one in the tooltip |

Checked in code and unit tests, not in a browser: Simple/Advanced switches size the main window
against the screen's work area every time, never with a minimum larger than it (`main_fit`,
`cargo test`); `fitScreen` leaves full-screen and maximized windows alone. Manage relay reads with
one relay that never answers: `apps/desktop/e2e/relay-timing.mjs`.

## Round 3 (2026-11-08)

A second blind review found two cut-off controls the tool had passed: the Manage Diagnostics table
(flagged by `hscroll`, merged anyway) and the technician app's "Send to support" (the browser mock
had no `diag_info`, and a label spilling out of a button with `overflow: visible` was not checked).
The desktop mock now answers `diag_info` and `diag_upload` like a real install (long relay names,
a failing route), the new scenarios `settings-diag-sent` and `settings-diag-error` fill the code
and send, and the `label-spill` check was added. Before the fix the tool flags Send to support at
every size ("label 34px tall in a 28px control"); after it, 0 issues. The new check also found the
Recent "More" menu item wrapping.

| Where | Size | What was wrong | Fix |
| --- | --- | --- | --- |
| Manage Diagnostics, Active and Archived | 900×600, 911×512 | 721 px table in a 646 / 657 px box: Delete past the card edge | four columns: note under the code, closed date under created, each upload with its own Open button, Close and Delete as icon buttons (label in the tooltip) in a column that does not shrink |
| Tether Settings › Diagnostics, Send to support | every size | the code field took the whole row (`w-full` of the field beat `w-40`), the label wrapped and "support" fell out of the button | field 10 rem, neither field nor button shrinks, label on one line |
| Tether Recent, More menu | every size | "Remove from recents" wrapped in a 36 px item | menu as wide as its item, label on one line |

Final run: Manage 220, TetherQS 52, Tether 358 state × size × theme combinations, 0 issues
(Tether's More menu re-checked after its fix). The real technician app (debug build, over CDP, 5
sizes × 2 themes) shows Send to support whole with the error below it.
