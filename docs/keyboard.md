# Keyboard (technician app)

How keys typed in a session window reach the remote computer. Code:
`apps/desktop/src-tauri/src/keys.rs` (mapping, hook, release), `apps/desktop/src/pages/Session.tsx`
(webview key events, **Keyboard** menu in the session toolbar). The core sends keys in RustDesk's
*map* mode: the physical key position travels, the remote computer's own layout decides the
character.

## Two ways in, one mapping

| Path | When | What it sees |
| --- | --- | --- |
| Webview (`keydown`/`keyup` → `session_key`) | always; the only path on macOS and when **Send system keys** is off | every key the window gets; *not* Win, Alt+Space, Alt+Tab, Ctrl+Esc… (Windows or other tools take those first) |
| System-key hook (Windows `WH_KEYBOARD_LL`, rdev) | Windows, while a session window has the focus, **Send system keys** is on and no text field (chat) has the focus | every key, before Windows, PowerToys or the Start menu |

Both feed the same per-session mapper. While the hook captures, the webview's copies are dropped,
so nothing is sent twice.

## PC ↔ Mac shortcuts

Chosen from the remote computer's platform (peer info). Toggle per session in **Keyboard**:
*Windows shortcuts on the Mac* / *Mac shortcuts on the PC* (on by default when the platforms
differ; off = keys as on the other keyboard, e.g. for a terminal). Same idea as MirrorMagic: the
modifier keys are swapped, not each shortcut.

PC technician → Mac:

| Press | Mac gets |
| --- | --- |
| Ctrl (left/right) | ⌘ — so Ctrl+C, V, X, A, Z, S, F, W, T, N, P, R, L, Q… work as on Windows |
| Win key | Control |
| Alt | Option |
| Ctrl+Y | ⇧⌘Z (redo) |
| Ctrl+← / → | ⌥← / → (word) |
| Ctrl+Backspace / Delete | ⌥Backspace / ⌥Delete (word) |
| Home / End (with Shift: select) | ⌘← / ⌘→ (line start / end) |
| Ctrl+Home / End | ⌘↑ / ⌘↓ (document start / end) |
| Alt+Tab (only with **Send Alt+Tab**) | ⌘Tab, the switcher stays open while Alt is held |
| Alt+F4 | ⌘Q |
| AltGr+key (German and other AltGr layouts: @ € [ ] { } \ \| ~ …) | the character as text, whatever the Mac's layout |
| Ctrl+Alt+Del | nothing; **Keyboard → Send ⌘⌥Esc** opens Force Quit |

Mac technician → PC: ⌘ → Ctrl, Control → Win, Option → Alt, ⌘← / → → Home / End,
⌘↑ / ↓ → Ctrl+Home / End, ⌥← / → → Ctrl+← / →, ⌥Backspace → Ctrl+Backspace, ⌘Q → Alt+F4,
⌘Tab → Alt+Tab (if the Mac lets it through, see below), Option+key giving @ € [ ] { } \ | ~ → that
character as text.

AltGr detection: Windows sends a fake left Ctrl right before right Alt on AltGr layouts. A right
Alt pressed directly after a left Ctrl is AltGr (the Ctrl is lifted again on the Mac, a ⌘ tap does
nothing there); a right Alt without it (US layout) is Option.

## System keys (Windows technician)

With **Send system keys** on (default), these go to the remote computer while the session window
is active: Win, Win+R / E / D / …, Alt+Space, Alt+Esc, Ctrl+Esc, Print, Ctrl+Shift+Esc,
the context-menu key, and Alt+Tab if **Send Alt+Tab** is on.

- The hook is installed when the session window gets the focus and removed when it loses it
  (another window, minimized, closed), when a text field in the window (chat) gets the focus, and
  when the setting is turned off.
- **Local escape: Ctrl+Alt+Home** gives the keys back to this PC (the hook is removed) until you
  click into the remote picture again or the window gets the focus again. It also moves the
  keyboard to the session toolbar, see [Local keyboard](#local-keyboard-ctrlalthome).
- **Alt+Tab** with *Send Alt+Tab* off (default): switches windows on this PC. The remote Alt is
  released, the hook removed, and Alt+Tab handed to Windows.
- Ctrl+Alt+Del and Win+L are always handled by Windows itself (no program can capture them); use
  **Remote actions → Send Ctrl+Alt+Del**.
- Caps Lock and Num Lock also toggle locally; their state goes with every key.

## Local keyboard (Ctrl+Alt+Home)

Every key typed in a session window goes to the remote computer, so the toolbar could only be
used with the mouse. **Ctrl+Alt+Home** switches the keyboard to this window (all platforms, with
or without the hook):

- The toolbar is shown and its first button gets the focus; a short message says so ("The
  keyboard now works this toolbar. Esc gives it back to the remote computer."). No key reaches
  the remote computer while this lasts: keys pressed in that mode are never sent, neither press
  nor release; keys that were already down get their release. While the keyboard is local the
  toolbar shows "Keyboard: this window, Esc to return".
- **Tab** and the **arrow keys** move along the toolbar and through an open menu, **Enter** /
  **Space** press the focused button.
- **Escape** closes an open confirmation (lock, restart) first, then an open menu, then gives the
  keyboard back to the remote computer. A click into the picture gives it back too.
- With **Send system keys** on (Windows) the hook swallows the chord, so the page never sees it:
  the hook tells that session window through the `session` event `local_escape`
  (`LOCAL_ESCAPE_EVENT` in `keys.rs`, handled in `Session.tsx`). Without the hook the page sees
  the chord itself.

## Keys that stayed pressed ("keyboard only partly works")

A key whose release the window never sees stayed pressed on the remote computer: Alt+Tab or the
Win key on this PC, Alt+Space opening PowerToys Run or Windows search, any popup taking the focus.
The remote then saw Alt or Win held with every later key, and the core ignored the next press of
that modifier as "auto-repeat". Now:

- when the session window loses the focus, every key still held is released on the remote side
  (also when a text field takes the focus);
- the keys that caused it (Win, Alt+Space…) no longer leave the window while system keys are on;
- with the hook, typing does not depend on the webview having the keyboard focus inside the
  window (a click on the title bar or a restore from the taskbar is enough).

If the person at the remote computer turns off **Control mouse and keyboard**, the session shows
"View only: the person at the remote computer has turned off control of mouse and keyboard." The
viewer never sets a crosshair cursor itself: the cursor over the picture is the remote computer's.

## macOS technician app

No hook yet: macOS keeps ⌘Tab, ⌘Space, ⌘Q (quits Tether) and Mission Control keys. Capturing them
needs a `CGEventTap` with the Accessibility (or Input Monitoring) permission for the technician app
— follow-up. The mapping and the release on focus loss work on macOS too (compile-unverified until
the next Mac build).

## Tests

- Unit tests (`cargo test keys` in `apps/desktop/src-tauri`): mapping tables in both directions,
  AltGr, Alt+Tab → ⌘Tab, release, the hook's on/off state machine and its key decisions.
- `apps/desktop/e2e/keyboard-e2e.mjs`: host and viewer debug builds on a hidden desktop; keys are
  sent into the session page with CDP `Input.dispatchKeyEvent`; the host runs with
  `TETHER_DEV_KEYLOG=<file>` (debug builds only: logs received keys, injects nothing, ignores
  mouse events). The viewer pretends the host is a Mac (`TETHER_DEV_PEER_PLATFORM`) and installs
  no hook (`TETHER_DEV_NOHOOK`).
- The hook itself needs real key presses: manual check below.

### Manual check (Windows technician PC, real keyboard)

1. Connect to a Windows PC. Click into the picture. Press **Win**: the remote Start menu opens,
   not the local one. **Win+R**: the remote Run box. **Alt+Space**: the remote window menu (not
   PowerToys Run / local search). **Ctrl+Esc**, **Alt+Esc**, **Print** likewise.
2. **Alt+Tab**: switches windows on this PC. Back in the session (click or Alt+Tab), type: no key
   is stuck (letters are letters, not Alt shortcuts). Turn on **Keyboard → Send Alt+Tab**: Alt+Tab
   now switches on the remote PC.
3. **Ctrl+Alt+Home**: the toolbar appears with its first button focused and the message "The
   keyboard now works this toolbar…". Tab / arrow keys move along it, Enter opens a menu, Escape
   closes the menu, a second Escape returns the keyboard (typing reaches the remote PC again).
   **Ctrl+Alt+Home**, then Win: the local Start menu opens. Click into the picture: Win goes to
   the remote again.
4. Open the chat and type in its field: the text appears in the field, nothing on the remote PC.
5. Turn off **Keyboard → Send system keys**: Win opens the local Start menu.
6. With a Mac: Ctrl+C / Ctrl+V copy and paste, Home/End jump to line start/end, Ctrl+← jumps a
   word, AltGr+Q types @ (German PC keyboard), Win key acts as Control. Turn off *Windows shortcuts
   on the Mac*: Ctrl+C in Terminal interrupts.
