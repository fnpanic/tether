# Session recording

## Where are my recordings?

- **On the main window, under "Recordings"** (below Recent): every recording with date, device name and
  ID, length and size. Each row has **Play** (opens in your default video player), **Show in folder**
  and **Delete**. **Open folder** opens the folder itself.
- **When a recording stops**, every open Tether window shows a card "Recording saved" with the file name,
  **Open** and **Show in folder**. With an upload configured it also shows "Uploading to …", then
  "Uploaded" or "Upload failed" (Tether retries at the next session or start).
- **The folder** is `%USERPROFILE%\Videos\Tether` on Windows and `~/Movies/Tether` on macOS, unless
  your organisation set `recording.folder`. **Settings → Recording** shows the folder in use, lets you
  pick another one (**Change…**, **Use default**) unless policy locks it, and shows the recording mode,
  upload target and retention that policy sets.
- **File names** read `2026-11-05_14-03-22_<device name>_<ID>.webm` (start date and time, the remote
  device's name, its Tether ID). A second monitor adds `_display1`. They are standard WebM (VP8/VP9/AV1)
  or MP4 (H.264/H.265) files that any player opens. Files from older versions keep the core's name
  `outgoing_<ID>_<time>_display0_<codec>.webm` and are listed too.
- **During recording** the session shows a red "● REC 00:42" pill at the top left and the camera button
  in the toolbar turns red. While the other person has not answered yet, the camera button pulses.
- **The other person** is always asked first ("Recording request" in Tether, a dialog in TetherQS) and,
  once they allow it, sees "Recording allowed" in TetherQS for the rest of the session.

## How others do it

| Product | Where files go | After recording | Format and name | Admin control |
| --- | --- | --- | --- | --- |
| TeamViewer | Asks where to save at session end, or a set "session recording directory" | Built-in player and converter (Extras → Play or convert recorded session) | Proprietary `.tvs`, `Device (ID)_[date] [time]` | Policies: force auto-record, fixed folder |
| AnyDesk | `%homepath%\Videos\AnyDesk`, Settings → Recording → custom path | Built-in player (double-click or main menu) | Proprietary `.anydesk`, `<direction> <peer> (<ID>)-<own> (<ID>)` | Auto-record in/out; always local |
| RustDesk 1.4 | `%USERPROFILE%\Videos\RustDesk`, Settings → General → Recording → "Video save directory" | Nothing in-app; open the folder | WebM/MP4, `outgoing_<ID>_<time>_display0_<codec>` | `allow-auto-record-*`, `hide-recording-button` |
| Splashtop | `Documents\Splashtop Business`, path and size limit set in the web portal | Cloud recording (add-on): web player in the session logs | WebM (Windows) / MP4 (Mac) | Per-OS auto-record and storage limit |
| ScreenConnect | On the server ("session capture") | Admin → Audit → Download Video (transcoded to AVI) | Server format | Server-side auto-capture |
| BeyondTrust | On the appliance or cloud | Session reports with playback, by permission | Server format | Always-on by admin, up to 90 days retention |
| Chrome Remote Desktop | No recording | | | |

Tether follows the simple ones: standard files, a fixed default folder, a clear name and an in-app
list instead of a custom player. Sources: [TeamViewer Classic](https://www.teamviewer.com/en-us/global/support/knowledge-base/teamviewer-classic/remote-control/in-session-features/record-a-remote-session),
[TeamViewer policies](https://www.teamviewer.com/en-us/global/support/knowledge-base/teamviewer-remote/devices/session-recording-policies/),
[AnyDesk](https://support.anydesk.com/docs/session-recording),
[RustDesk settings](https://rustdesk.com/docs/en/self-host/client-configuration/advanced-settings/) and
[record.rs](https://github.com/rustdesk/rustdesk/blob/master/libs/scrap/src/common/record.rs),
[Splashtop](https://support-splashtopbusiness.splashtop.com/hc/articles/360021851931),
[ScreenConnect audit page](https://docs.connectwise.com/ConnectWise_ScreenConnect_Documentation/Get_started/Administration_page/Audit_page),
[BeyondTrust auditing](https://www.beyondtrust.com/docs/remote-support/getting-started/deployment/security/auditing.htm).
Most vendors do not document an after-stop notice or the remote-side indicator.

## How it works

The technician (viewer) records the remote screen locally with the core's recorder (WebM/MP4, the
same encoded stream it receives, no re-encoding). The remote user always has to consent first.
Frames the webview decodes itself (WebCodecs passthrough) never reach the core's video threads, so
`tauri_ui` feeds them to a recorder of its own (`TauriHandler::record_passthrough`); the viewer asks
for a key frame when recording starts so each file begins with one.

### Policy keys

| Key | Default | Meaning |
| --- | --- | --- |
| `recording.mode` | `on-request` | `off` hides the toolbar button and refuses `session_record`; `on-request` shows the button; `always` requests recording as soon as each session (and each reconnect) is established. |
| `recording.folder` | `%USERPROFILE%\Videos\Tether` | Where recordings are written. `%VAR%` is expanded. When the feature is not locked, a folder picked in Settings → Recording (local option `tether-recording-folder`) wins. |
| `recording.upload` | `none` | `none`, `unc` (copy to a share or local folder) or `https-put`. |
| `recording.upload-target` | | UNC/local folder, or base HTTPS URL (the file is PUT to `<target>/<filename>`). |
| `recording.upload-token` | | Bearer token for `https-put`. |
| `recording.keep-local` | `Y` | `N` deletes the local file after a successful upload. |
| `recording.max-days` | `0` | Delete local recordings older than this many days (`0` = never). |
| `recording.consent-text` | "The technician wants to record this session. Allow?" | Question shown to the remote user. |
| `connect.reconnect-minutes` | `10` | How long a dropped session keeps reconnecting (see below). |

The core options `allow-auto-record-incoming/outgoing` stay in the policy floor: Tether never records
silently, `always` still goes through consent.

### Consent

1. Every incoming connection on a Tether host starts with its `recording` permission **off**,
   regardless of `enable-record-session` (`server/connection.rs`, `#[cfg(feature = "tauri")]`).
   The host tells the viewer at connect time, so the viewer's recorder is gated off
   (`record_state && record_permission` in `client/io_loop.rs`) before the first frame.
2. The viewer clicks Record (`session_record { key, start: true }`). Without the permission the
   viewer sends `Misc.client_record_status = true` as a request (`io_loop.rs` hook); no frame is
   recorded.
3. The host's connection forwards it to its connection manager as
   `ipc::Data::SwitchPermission { name: "recording-request" }`; the CM emits the event
   `record_consent_request` with `{ conn_id: number, peer_name: string, text: string }`
   (`tauri_ui/cm.rs`). In the desktop app it arrives on the `cm` channel as
   `{ name: "record_consent_request", payload }`.
4. The user answers with the command `cm_record_consent { connId: number, allow: boolean }`
   (`librustdesk::ui::cm::record_consent`). Allow turns the connection's `recording` permission on;
   the viewer then starts its recorder and shows the red REC pill with a timer. Deny sends the
   permission as off again; the viewer shows "Recording declined by the other party".
5. The host can revoke at any time with the "Record the session" toggle; the viewer's recorder stops.

Apps embedding the CM call `cm::set_record_consent_text` once (the desktop app does it via
`cm_record_consent_init` from `recording.consent-text`).

Limits: consent needs an interactive user on the host (an unattended host never answers, so nothing
is recorded). A modified viewer could still capture the frames it receives; consent is enforced by
Tether's own viewer. A reconnect is a new connection and asks again.

### Storage, upload and retention

- Before recording starts the app points the core's `video-save-directory` at `recording.folder`.
- When a recording stops (or the session drops, or the next session window opens) the app collects
  the recordings closed since the recording started into a small queue
  (`<config dir>/recording-uploads.json`) and uploads them. Failed uploads stay queued and are
  retried on the next run. UNC copies go to `<name>.part` first and are renamed when complete.
- After a successful upload the local file is deleted if `recording.keep-local = N`.
- Files older than `recording.max-days` are deleted from the folder, except files still waiting for
  upload.
- A closed file is renamed from the core's `outgoing_<id>_<time>_display<n>_<codec>.<ext>` to
  `<YYYY-mm-dd>_<HH-MM-SS>_<peer host name>_<id>[_display<n>].<ext>` (host name from the saved peer
  info, letters, digits, `-` and `.` only) before it is queued, so the upload has the new name too.
- The app emits the event `recordings` to every window: `{ saved: Recording[], upload }` when files
  are closed, `{ uploaded: string[], failed: [name, error][] }` after an upload attempt.
- Commands: `recordings_list` (folder, `custom`, `locked`, items newest first), `recordings_open
  { name?, reveal? }`, `recordings_delete { name }`, `recordings_pick_folder`, `recordings_reset_folder`.
  They take a bare file name, never a path: anything that is not a recording directly inside the
  recordings folder (separators, `..`, other files, links that lead out) is refused.

### Reconnect

A session that was working and fails with a transport error (core `msgbox` with `retry = true`,
e.g. "Reset by the peer", "Timeout", relay drop) reconnects on its own: the last frame stays visible,
dimmed, with "Reconnecting… (attempt n)". Attempts start at once, then wait 0.25, 0.5, 1, 2, 4, 8 s, then every 10 s
(`lib/reconnect.ts`); every attempt goes through the relay. After `connect.reconnect-minutes` the error
screen with a manual Retry is shown. Wrong password, rejected/closed by the other side, policy and
version errors are never retried; "offline" and "could not be end-to-end encrypted" (a restarted
peer that has not re-registered its key yet) never start a reconnect but do not stop one already
running. Every attempt still refuses an unencrypted connection. The core keeps the hashed password, so no password is asked
again unless the host changed it. The logic is in `apps/desktop/src/lib/reconnect.ts`.
