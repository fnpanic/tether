# UI layout audit

Every window of the three apps checked for content that is cut off, half visible, overflowing,
overlapping or unreachable; rules in [DESIGN.md](../DESIGN.md). Last run 2026-11-08.

Run it again: `node tools/ui-audit.mjs <desktop|quick|manage> <label> [scenario-regex]` (after
`npm ci` in the app). It starts the app's Vite server, drives it in Edge (mock mode; TetherQS gets a
stand-in for the Tauri IPC), takes a screenshot of each state and checks each one in the page:
elements clipped by a container that does not scroll, elements outside the window, text cut off
without an ellipsis, interactive elements covered by another element of the same layer, and a
page that scrolls sideways. Results: `findings.json` next to the screenshots.

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
