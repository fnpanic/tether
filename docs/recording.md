# Session recording

The technician (viewer) records the remote screen locally with the core's recorder (WebM/MP4, the
same encoded stream it receives, no re-encoding). The remote user always has to consent first.
Frames the webview decodes itself (WebCodecs passthrough) never reach the core's video threads, so
`tauri_ui` feeds them to a recorder of its own (`TauriHandler::record_passthrough`); the viewer asks
for a key frame when recording starts so each file begins with one.

## Policy keys

| Key | Default | Meaning |
| --- | --- | --- |
| `recording.mode` | `on-request` | `off` hides the toolbar button and refuses `session_record`; `on-request` shows the button; `always` requests recording as soon as each session (and each reconnect) is established. |
| `recording.folder` | `%USERPROFILE%\Videos\Tether` | Where recordings are written. `%VAR%` is expanded. |
| `recording.upload` | `none` | `none`, `unc` (copy to a share or local folder) or `https-put`. |
| `recording.upload-target` | | UNC/local folder, or base HTTPS URL (the file is PUT to `<target>/<filename>`). |
| `recording.upload-token` | | Bearer token for `https-put`. |
| `recording.keep-local` | `Y` | `N` deletes the local file after a successful upload. |
| `recording.max-days` | `0` | Delete local recordings older than this many days (`0` = never). |
| `recording.consent-text` | "The technician wants to record this session. Allow?" | Question shown to the remote user. |
| `connect.reconnect-minutes` | `10` | How long a dropped session keeps reconnecting (see below). |

The core options `allow-auto-record-incoming/outgoing` stay in the policy floor: Tether never records
silently, `always` still goes through consent.

## Consent

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

## Storage, upload and retention

- Before recording starts the app points the core's `video-save-directory` at `recording.folder`.
- When a recording stops (or the session drops, or the next session window opens) the app collects
  the recordings closed since the recording started into a small queue
  (`<config dir>/recording-uploads.json`) and uploads them. Failed uploads stay queued and are
  retried on the next run. UNC copies go to `<name>.part` first and are renamed when complete.
- After a successful upload the local file is deleted if `recording.keep-local = N`.
- Files older than `recording.max-days` are deleted from the folder, except files still waiting for
  upload.

## Reconnect

A session that was working and fails with a transport error (core `msgbox` with `retry = true`,
e.g. "Reset by the peer", "Timeout", relay drop) reconnects on its own: the last frame stays visible,
dimmed, with "Reconnecting… (attempt n)". Attempts wait 0.5, 1, 2, 4, 8 s, then every 10 s; from the
third attempt the connection is forced through the relay. After `connect.reconnect-minutes` the error
screen with a manual Retry is shown. Wrong password, rejected/closed by the other side, policy and
version errors are never retried; "offline" and "could not be end-to-end encrypted" (a restarted
peer that has not re-registered its key yet) never start a reconnect but do not stop one already
running. Every attempt still refuses an unencrypted connection. The core keeps the hashed password, so no password is asked
again unless the host changed it. The logic is in `apps/desktop/src/lib/reconnect.ts`.
