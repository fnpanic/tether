//! Viewer keyboard: shortcut mapping between PC and Mac, the system-key hook (Windows) and the
//! release of held keys when a session window loses focus. See docs/keyboard.md.
//!
//! Every key of a session goes through one per-session [`Mapper`]: from the webview (`session_key`) or, while the
//! hook is on, from the low-level hook. Both feed the same per-session [`Mapper`], so a key pressed
//! on one path and released on the other is still released on the remote side.

use std::{
    collections::HashMap,
    sync::{LazyLock, Mutex},
};

use rdev::Key;

// ------------------------------------------------------------------------------------- mapping

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    /// Same keyboard family on both sides: keys pass unchanged.
    Same,
    /// Windows/Linux technician, Mac host.
    PcToMac,
    /// Mac technician, Windows/Linux host.
    MacToPc,
}

impl Dir {
    pub fn new(viewer_mac: bool, peer_platform: &str) -> Dir {
        let peer_mac = peer_platform.to_lowercase().contains("mac");
        match (viewer_mac, peer_mac) {
            (false, true) => Dir::PcToMac,
            (true, false) if !peer_platform.is_empty() => Dir::MacToPc,
            _ => Dir::Same,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Out {
    Key(Key, bool),
    Text(String),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    None,
    Ctrl,
    Alt,
    Meta,
}

/// (direction, held modifier class, key) → modifiers + key sent instead. `sticky`: the held
/// modifier stays replaced until it is released (Alt+Tab → Cmd+Tab keeps the switcher open).
struct Rule {
    dir: Dir,
    class: Class,
    key: Key,
    mods: &'static [Key],
    to: Key,
    sticky: bool,
}

const fn r(dir: Dir, class: Class, key: Key, mods: &'static [Key], to: Key) -> Rule {
    Rule { dir, class, key, mods, to, sticky: false }
}

use Class as C;
use Dir::{MacToPc as M2P, PcToMac as P2M};
use Key as K;

const RULES: &[Rule] = &[
    // PC → Mac (Ctrl already acts as Cmd)
    r(P2M, C::Ctrl, K::KeyY, &[K::MetaLeft, K::ShiftLeft], K::KeyZ), // redo
    r(P2M, C::Ctrl, K::LeftArrow, &[K::Alt], K::LeftArrow),          // word jumps
    r(P2M, C::Ctrl, K::RightArrow, &[K::Alt], K::RightArrow),
    r(P2M, C::Ctrl, K::Backspace, &[K::Alt], K::Backspace), // delete word
    r(P2M, C::Ctrl, K::Delete, &[K::Alt], K::Delete),
    r(P2M, C::None, K::Home, &[K::MetaLeft], K::LeftArrow), // line start / end
    r(P2M, C::None, K::End, &[K::MetaLeft], K::RightArrow),
    r(P2M, C::Ctrl, K::Home, &[K::MetaLeft], K::UpArrow), // document start / end
    r(P2M, C::Ctrl, K::End, &[K::MetaLeft], K::DownArrow),
    r(P2M, C::Alt, K::F4, &[K::MetaLeft], K::KeyQ),
    Rule { dir: P2M, class: C::Alt, key: K::Tab, mods: &[K::MetaLeft], to: K::Tab, sticky: true },
    // Mac → PC (Cmd already acts as Ctrl)
    r(M2P, C::Meta, K::LeftArrow, &[], K::Home),
    r(M2P, C::Meta, K::RightArrow, &[], K::End),
    r(M2P, C::Meta, K::UpArrow, &[K::ControlLeft], K::Home),
    r(M2P, C::Meta, K::DownArrow, &[K::ControlLeft], K::End),
    r(M2P, C::Alt, K::LeftArrow, &[K::ControlLeft], K::LeftArrow),
    r(M2P, C::Alt, K::RightArrow, &[K::ControlLeft], K::RightArrow),
    r(M2P, C::Alt, K::Backspace, &[K::ControlLeft], K::Backspace),
    r(M2P, C::Meta, K::KeyQ, &[K::Alt], K::F4),
    Rule { dir: M2P, class: C::Meta, key: K::Tab, mods: &[K::Alt], to: K::Tab, sticky: true },
];

/// Characters a Mac types with Option and a PC with AltGr; sent as text, so the other side's
/// layout does not matter.
const ALTGR_CHARS: &str = "@€[]{}\\|~";

fn is_mod(k: Key) -> bool {
    matches!(
        k,
        K::ShiftLeft | K::ShiftRight | K::ControlLeft | K::ControlRight | K::Alt | K::AltGr | K::MetaLeft | K::MetaRight
    )
}

/// Translates physical key events of one session into what is sent to the remote side.
pub struct Mapper {
    pub dir: Dir,
    /// The per-session toggle: Ctrl acts as Cmd (Win key as Control) and the rules above.
    pub shortcuts: bool,
    held: Vec<Key>,
    /// What each held physical key is currently pressed as on the remote side.
    sent: HashMap<Key, Vec<Key>>,
    /// Modifiers lifted for the duration of a rule key, pressed again on its release.
    restore: HashMap<Key, Vec<Key>>,
    last_down: Option<Key>,
    /// Right Alt is AltGr (Windows sent its fake left Ctrl right before it).
    altgr: bool,
}

impl Mapper {
    pub fn new(dir: Dir) -> Self {
        Mapper { dir, shortcuts: dir != Dir::Same, held: vec![], sent: HashMap::new(), restore: HashMap::new(), last_down: None, altgr: false }
    }

    fn swapping(&self) -> bool {
        self.shortcuts && self.dir != Dir::Same
    }

    /// The modifier as sent: Ctrl ↔ Cmd/Win when swapping.
    fn base(&self, k: Key) -> Key {
        if !self.swapping() {
            return k;
        }
        match k {
            K::ControlLeft => K::MetaLeft,
            K::ControlRight => K::MetaRight,
            K::MetaLeft => K::ControlLeft,
            K::MetaRight => K::ControlRight,
            k => k,
        }
    }

    fn class_of(&self, k: Key) -> Option<Class> {
        match k {
            K::ControlLeft | K::ControlRight => Some(C::Ctrl),
            K::Alt => Some(C::Alt),
            K::AltGr if !self.altgr => Some(C::Alt),
            K::MetaLeft | K::MetaRight => Some(C::Meta),
            _ => None,
        }
    }

    /// The one modifier class held (Shift ignored); None if none, also None on mixes.
    fn class(&self) -> Option<Class> {
        let mut c = C::None;
        for &k in &self.held {
            match self.class_of(k) {
                Some(x) if c == C::None || c == x => c = x,
                Some(_) => return None,
                None => {}
            }
        }
        Some(c)
    }

    pub fn alt_held(&self) -> bool {
        self.held.iter().any(|&k| self.class_of(k) == Some(C::Alt))
    }

    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn altgr_active(&self) -> bool {
        self.altgr
    }

    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn ctrl_held(&self) -> bool {
        self.held.iter().any(|&k| matches!(k, K::ControlLeft | K::ControlRight))
    }

    /// Lift the outputs of held modifiers of `class` (returns the physical keys lifted).
    fn lift(&mut self, class: Class, out: &mut Vec<Out>) -> Vec<Key> {
        let keys: Vec<Key> = self.held.iter().copied().filter(|&k| self.class_of(k) == Some(class)).collect();
        for k in &keys {
            for s in self.sent.get_mut(k).map(std::mem::take).unwrap_or_default().into_iter().rev() {
                out.push(Out::Key(s, false));
            }
        }
        keys
    }

    /// One physical key event. `ch` is the character the local layout produces ("" if none).
    pub fn map(&mut self, k: Key, down: bool, ch: &str) -> Vec<Out> {
        let mut out = vec![];
        if !down {
            self.held.retain(|&h| h != k);
            if k == K::AltGr {
                self.altgr = false;
            }
            self.last_down = None;
            for s in self.sent.remove(&k).unwrap_or_default().into_iter().rev() {
                out.push(Out::Key(s, false));
            }
            for m in self.restore.remove(&k).unwrap_or_default() {
                if self.held.contains(&m) && self.sent.get(&m).map_or(true, |s| s.is_empty()) {
                    let b = self.base(m);
                    self.sent.insert(m, vec![b]);
                    out.push(Out::Key(b, true));
                }
            }
            return out;
        }
        if let Some(s) = self.sent.get(&k) {
            // auto-repeat: repeat the main key only (or the text)
            return match s.last() {
                Some(&x) => vec![Out::Key(x, true)],
                None if !ch.is_empty() && !is_mod(k) && (self.altgr || self.restore.contains_key(&k)) => vec![Out::Text(ch.into())],
                None => vec![],
            };
        }
        let prev = self.last_down.replace(k);
        self.held.push(k);

        // Windows AltGr = fake left Ctrl + right Alt. Towards a Mac it would be Cmd+Option: undo
        // the Ctrl and type AltGr characters as text instead.
        if self.dir == Dir::PcToMac && k == K::AltGr && prev == Some(K::ControlLeft) && self.held.contains(&K::ControlLeft) {
            self.altgr = true;
            for s in self.sent.get_mut(&K::ControlLeft).map(std::mem::take).unwrap_or_default().into_iter().rev() {
                out.push(Out::Key(s, false));
            }
            self.sent.insert(k, vec![]);
            return out;
        }
        let text = !ch.is_empty() && !ch.chars().any(char::is_control) && !is_mod(k);
        if self.dir == Dir::PcToMac && self.altgr && text {
            self.sent.insert(k, vec![]);
            return vec![Out::Text(ch.into())];
        }
        if self.dir == Dir::MacToPc && self.alt_held() && text && ch.chars().count() == 1 && ALTGR_CHARS.contains(ch) {
            let lifted = self.lift(C::Alt, &mut out);
            self.restore.insert(k, lifted);
            self.sent.insert(k, vec![]);
            out.push(Out::Text(ch.into()));
            return out;
        }
        if is_mod(k) {
            let b = self.base(k);
            self.sent.insert(k, vec![b]);
            return vec![Out::Key(b, true)];
        }
        if self.swapping() {
            if let Some(class) = self.class() {
                if let Some(rule) = RULES.iter().find(|r| r.dir == self.dir && r.class == class && r.key == k) {
                    let already = self.held.iter().any(|m| self.class_of(*m) == Some(class) && self.sent.get(m).map(Vec::as_slice) == Some(rule.mods));
                    if rule.sticky && already {
                        // next Tab while the switcher is open: the modifier is already swapped
                        self.sent.insert(k, vec![rule.to]);
                        return vec![Out::Key(rule.to, true)];
                    }
                    let lifted = if class == C::None { vec![] } else { self.lift(class, &mut out) };
                    let mut keys: Vec<Key> = rule.mods.to_vec();
                    if rule.sticky && !lifted.is_empty() {
                        self.sent.insert(lifted[0], keys.clone());
                        keys.clear();
                    } else {
                        self.restore.insert(k, lifted);
                    }
                    for &m in rule.mods {
                        out.push(Out::Key(m, true));
                    }
                    keys.push(rule.to);
                    out.push(Out::Key(rule.to, true));
                    self.sent.insert(k, keys);
                    return out;
                }
            }
        }
        self.sent.insert(k, vec![k]);
        vec![Out::Key(k, true)]
    }

    /// Key-ups for everything still pressed on the remote side (focus lost, session closed).
    pub fn release_all(&mut self) -> Vec<Out> {
        let mut keys: Vec<(Key, Vec<Key>)> = self.sent.drain().collect();
        keys.sort_by_key(|(k, _)| is_mod(*k)); // other keys first, then modifiers
        self.held.clear();
        self.restore.clear();
        self.last_down = None;
        self.altgr = false;
        keys.into_iter().flat_map(|(_, s)| s.into_iter().rev().map(|x| Out::Key(x, false))).collect()
    }
}

// ------------------------------------------------------------------------------- session state

struct Sess {
    mapper: Mapper,
    /// Toolbar toggles.
    system_keys: bool,
    alt_tab_remote: bool,
    /// Set by the webview while a text field of the session window (chat) has the focus.
    paused: bool,
    /// Set by the local escape and a local Alt+Tab; cleared on focus or a click into the picture.
    released: bool,
    /// Key-downs the hook sent and also let through locally (lock keys): the webview skips them.
    passed: Vec<Key>,
}

#[derive(Default)]
struct State {
    sessions: HashMap<String, Sess>,
    /// Session window with the OS focus.
    focused: Option<String>,
    hook_installed: bool,
}

impl State {
    /// The session the system-key hook should capture for, if any.
    fn hook_target(&self) -> Option<&str> {
        let k = self.focused.as_deref()?;
        let s = self.sessions.get(k)?;
        (s.system_keys && !s.paused && !s.released && cfg!(windows)).then_some(k)
    }

    fn sess(&mut self, key: &str, peer: &str) -> &mut Sess {
        let dir = Dir::new(cfg!(target_os = "macos"), peer);
        let s = self.sessions.entry(key.to_owned()).or_insert_with(|| Sess {
            mapper: Mapper::new(dir),
            system_keys: true,
            alt_tab_remote: false,
            paused: false,
            released: false,
            passed: vec![],
        });
        // peer_info arrives after the window opened: pick the direction up once it is known
        if s.mapper.dir != dir && s.mapper.held.is_empty() {
            s.mapper = Mapper::new(dir);
        }
        s
    }
}

static STATE: LazyLock<Mutex<State>> = LazyLock::new(Default::default);

fn send(key: &str, outs: Vec<Out>, lock_modes: i32) {
    if outs.is_empty() {
        return;
    }
    let Some(s) = librustdesk::ui::session(key) else { return };
    for o in outs {
        match o {
            Out::Key(k, down) => {
                let hid = rdev::usb_hid_keycode_from_key(k).unwrap_or(0);
                if hid != 0 {
                    s.handle_flutter_key_event(hbb_common::SessionID::nil(), "map", "", hid as i32, lock_modes, down);
                }
            }
            Out::Text(t) => s.input_string(&t),
        }
    }
}

fn peer(key: &str) -> String {
    #[cfg(debug_assertions)]
    if let Ok(p) = std::env::var("TETHER_DEV_PEER_PLATFORM") {
        return p; // keyboard e2e: test the Mac mapping against a local Windows host
    }
    librustdesk::ui::session(key).map(|s| s.peer_platform()).unwrap_or_default()
}

/// A key from the webview. A key-down the hook took never gets here, so every key-down that does
/// is sent: also when Windows silently dropped a slow hook (dropping them here left the session
/// with no keyboard at all). Key-ups pass the hook and arrive here too; a second key-up of a key
/// the hook already released sends nothing. Lock keys pass the hook on the way down as well.
pub fn webview_input(key: &str, k: Key, down: bool, ch: &str, lock_modes: i32) {
    let peer = peer(key);
    let mut st = STATE.lock().unwrap();
    let s = st.sess(key, &peer);
    if down {
        if let Some(i) = s.passed.iter().position(|&p| p == k) {
            s.passed.remove(i);
            return;
        }
    }
    let outs = s.mapper.map(k, down, ch);
    drop(st);
    send(key, outs, lock_modes);
}

#[derive(serde::Serialize)]
pub struct Settings {
    /// "pc-mac", "mac-pc" or "" (same family).
    dir: &'static str,
    shortcuts: bool,
    system_keys: bool,
    alt_tab_remote: bool,
    /// The system-key hook exists on this platform (Windows).
    hook: bool,
}

/// Read and change the per-session keyboard settings; `pause`/`resume` come from the webview.
pub fn settings(key: &str, shortcuts: Option<bool>, system_keys: Option<bool>, alt_tab_remote: Option<bool>, pause: Option<bool>) -> Settings {
    let peer = peer(key);
    let mut st = STATE.lock().unwrap();
    let s = st.sess(key, &peer);
    let mut outs = vec![];
    if let Some(v) = shortcuts {
        if v != s.mapper.shortcuts {
            outs = s.mapper.release_all();
            s.mapper.shortcuts = v;
        }
    }
    if let Some(v) = system_keys {
        s.system_keys = v;
    }
    if let Some(v) = alt_tab_remote {
        s.alt_tab_remote = v;
    }
    if let Some(p) = pause {
        s.paused = p;
        if p {
            outs.extend(s.mapper.release_all());
        } else {
            s.released = false; // a click into the picture takes the keyboard back
        }
    }
    let r = Settings {
        dir: match s.mapper.dir {
            Dir::PcToMac => "pc-mac",
            Dir::MacToPc => "mac-pc",
            Dir::Same => "",
        },
        shortcuts: s.mapper.shortcuts,
        system_keys: s.system_keys,
        alt_tab_remote: s.alt_tab_remote,
        hook: cfg!(windows),
    };
    apply(&mut st);
    drop(st);
    send(key, outs, 0);
    r
}

/// Session window focus changed. Losing it releases every key still held on the remote side:
/// a key-up the window never sees (Alt+Tab, Win, a popup taking the focus) would otherwise stick.
pub fn focus(key: &str, focused: bool) {
    let mut st = STATE.lock().unwrap();
    let mut outs = vec![];
    if focused {
        st.focused = Some(key.to_owned());
        if let Some(s) = st.sessions.get_mut(key) {
            s.released = false;
        }
    } else {
        if st.focused.as_deref() == Some(key) {
            st.focused = None;
        }
        if let Some(s) = st.sessions.get_mut(key) {
            outs = s.mapper.release_all();
            s.passed.clear();
        }
    }
    apply(&mut st);
    drop(st);
    send(key, outs, 0);
}

/// Release held keys (webview: a text field took the focus).
pub fn release(key: &str) {
    let outs = STATE.lock().unwrap().sessions.get_mut(key).map(|s| s.mapper.release_all()).unwrap_or_default();
    send(key, outs, 0);
}

pub fn forget(key: &str) {
    let mut st = STATE.lock().unwrap();
    st.sessions.remove(key);
    if st.focused.as_deref() == Some(key) {
        st.focused = None;
    }
    apply(&mut st);
}

/// Cmd+Option+Esc (Force Quit) on a Mac, sent as is (not through the mapping).
pub fn mac_force_quit(key: &str) {
    let ks = [K::MetaLeft, K::Alt, K::Escape];
    let mut outs: Vec<Out> = ks.iter().map(|&k| Out::Key(k, true)).collect();
    outs.extend(ks.iter().rev().map(|&k| Out::Key(k, false)));
    send(key, outs, 0);
}

// ------------------------------------------------------------------------- system-key hook

/// Installs the hook the first time a session wants it and keeps it: rdev's `exit_grab` clears
/// its thread id before the old hook thread ends, and that thread then clears the id of a newer
/// hook, so focus in/out/in left hooks behind. An installed hook that does not capture passes
/// every key at once (`ours_in_front` before anything else).
#[cfg(windows)]
fn apply(st: &mut State) {
    if st.hook_installed || st.hook_target().is_none() {
        return;
    }
    st.hook_installed = true;
    std::env::set_var("KEYBOARD_ONLY", "y"); // no mouse hook
    rdev::set_event_popup(false);
    // rdev would call ToUnicodeEx for every key of every program (that breaks dead keys
    // elsewhere); the hook asks for the character itself, and only for AltGr towards a Mac.
    rdev::set_get_key_unicode(false);
    std::thread::spawn(|| {
        if let Err(e) = rdev::grab(hook) {
            hbb_common::log::error!("keyboard hook: {:?}", e);
            STATE.lock().unwrap().hook_installed = false;
        }
    });
}

/// The foreground window of the desktop that gets the keyboard belongs to this process. A Tether
/// whose session window is focused on another desktop (a second instance, a test run on a hidden
/// desktop) or that is not in front must never take keys: its hook is global.
#[cfg(windows)]
fn ours_in_front() -> bool {
    use windows_sys::Win32::System::StationsAndDesktops::{CloseDesktop, GetThreadDesktop, GetUserObjectInformationW, OpenInputDesktop, UOI_NAME};
    use windows_sys::Win32::System::Threading::{GetCurrentProcessId, GetCurrentThreadId};
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
    unsafe {
        let fg = GetForegroundWindow();
        if fg.is_null() {
            return false;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(fg, &mut pid);
        if pid != GetCurrentProcessId() {
            return false;
        }
        let name = |d| {
            let mut buf = [0u16; 128];
            let mut n = 0u32;
            let ok = GetUserObjectInformationW(d, UOI_NAME, buf.as_mut_ptr().cast(), (buf.len() * 2) as u32, &mut n) != 0;
            ok.then(|| String::from_utf16_lossy(&buf[..(n as usize / 2).saturating_sub(1)]))
        };
        let input = OpenInputDesktop(0, 0, 0x0001 /* DESKTOP_READOBJECTS */);
        if input.is_null() {
            return false; // secure desktop (UAC, lock screen)
        }
        let same = name(input).is_some() && name(input) == name(GetThreadDesktop(GetCurrentThreadId()));
        CloseDesktop(input);
        same
    }
}

/// The character the local layout makes for this key (AltGr text towards a Mac); asks without
/// changing the keyboard state (flag 4), so dead keys keep working.
#[cfg(windows)]
fn key_char(ev: &rdev::Event) -> String {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetKeyboardLayout, GetKeyboardState, ToUnicodeEx};
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
    unsafe {
        let mut state = [0u8; 256];
        if GetKeyboardState(state.as_mut_ptr()) == 0 {
            return String::new();
        }
        // the hook runs before the OS updates the key state: AltGr = Ctrl + Alt held
        state[0x11] |= 0x80;
        state[0x12] |= 0x80;
        let layout = GetKeyboardLayout(GetWindowThreadProcessId(GetForegroundWindow(), std::ptr::null_mut()));
        let mut buf = [0u16; 8];
        let n = ToUnicodeEx(ev.platform_code, ev.position_code, state.as_ptr(), buf.as_mut_ptr(), buf.len() as i32, 4, layout);
        if n > 0 { String::from_utf16_lossy(&buf[..n as usize]) } else { String::new() }
    }
}

#[cfg(not(windows))]
fn apply(_st: &mut State) {}

#[cfg(windows)]
fn lock_modes() -> i32 {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CAPITAL, VK_NUMLOCK};
    let on = |vk: u16| unsafe { GetKeyState(vk as i32) } & 1 != 0;
    (on(VK_CAPITAL) as i32) << 1 | (on(VK_NUMLOCK) as i32) << 2
}

/// What the hook does with one key while it captures (pure, see the tests).
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Debug, PartialEq, Eq)]
enum HookAction {
    /// Send to the remote side; `pass` lets the local OS see it too.
    Forward { pass: bool },
    /// Local escape (Ctrl+Alt+Home): stop capturing until the next click into the picture.
    Escape,
    /// Alt+Tab while Alt+Tab stays local: hand Alt+Tab to the local OS.
    LocalAltTab,
}

#[cfg_attr(not(windows), allow(dead_code))]
fn hook_action(m: &Mapper, k: Key, down: bool, alt_tab_remote: bool) -> HookAction {
    if down && k == K::Home && m.ctrl_held() && m.alt_held() {
        return HookAction::Escape;
    }
    if down && k == K::Tab && !alt_tab_remote && m.alt_held() {
        return HookAction::LocalAltTab;
    }
    // Lock keys also toggle locally (their state is sent with every key); releases pass so the
    // OS never keeps a key it saw go down before the hook started.
    HookAction::Forward { pass: !down || matches!(k, K::CapsLock | K::NumLock) }
}

#[cfg(windows)]
fn hook(ev: rdev::Event) -> Option<rdev::Event> {
    let (k, down) = match ev.event_type {
        rdev::EventType::KeyPress(k) => (k, true),
        rdev::EventType::KeyRelease(k) => (k, false),
        _ => return Some(ev),
    };
    if matches!(k, K::Unknown(_)) || !ours_in_front() {
        return Some(ev); // media and vendor keys, and every key while no window of ours is in front
    }
    let mut st = STATE.lock().unwrap();
    let Some(key) = st.hook_target().map(str::to_owned) else { return Some(ev) };
    let Some(s) = st.sessions.get_mut(&key) else { return Some(ev) };
    let (outs, keep) = match hook_action(&s.mapper, k, down, s.alt_tab_remote) {
        HookAction::Forward { pass } => {
            let ch = if down && s.mapper.dir == Dir::PcToMac && (k != K::AltGr) && s.mapper.altgr_active() { key_char(&ev) } else { String::new() };
            if pass && down {
                s.passed.push(k);
            }
            (s.mapper.map(k, down, &ch), pass)
        }
        HookAction::Escape => {
            s.released = true;
            (s.mapper.release_all(), false)
        }
        HookAction::LocalAltTab => {
            s.released = true;
            // The OS never saw this Alt go down: press it, then Tab, for the local switcher.
            std::thread::spawn(|| {
                let _ = rdev::simulate(&rdev::EventType::KeyPress(K::Alt));
                let _ = rdev::simulate(&rdev::EventType::KeyPress(K::Tab));
            });
            (s.mapper.release_all(), false)
        }
    };
    apply(&mut st);
    drop(st);
    send(&key, outs, lock_modes());
    keep.then_some(ev)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(v: &[(Key, bool)]) -> Vec<Out> {
        v.iter().map(|&(k, d)| Out::Key(k, d)).collect()
    }

    #[test]
    fn direction_from_platforms() {
        assert_eq!(Dir::new(false, "Mac OS"), Dir::PcToMac);
        assert_eq!(Dir::new(true, "Windows"), Dir::MacToPc);
        assert_eq!(Dir::new(true, "Linux"), Dir::MacToPc);
        assert_eq!(Dir::new(false, "Windows"), Dir::Same);
        assert_eq!(Dir::new(true, "Mac OS"), Dir::Same);
        assert_eq!(Dir::new(true, ""), Dir::Same); // peer not known yet
        assert!(!Mapper::new(Dir::Same).shortcuts);
    }

    #[test]
    fn same_platform_passes_keys() {
        let mut m = Mapper::new(Dir::Same);
        assert_eq!(m.map(K::ControlLeft, true, ""), keys(&[(K::ControlLeft, true)]));
        assert_eq!(m.map(K::KeyC, true, "c"), keys(&[(K::KeyC, true)]));
        assert_eq!(m.map(K::KeyC, false, ""), keys(&[(K::KeyC, false)]));
        assert_eq!(m.map(K::Home, true, ""), keys(&[(K::Home, true)]));
        // AltGr stays AltGr for a Windows host (same layout logic there)
        m.map(K::AltGr, true, "");
        assert_eq!(m.map(K::KeyQ, true, "@"), keys(&[(K::KeyQ, true)]));
    }

    #[test]
    fn pc_to_mac_ctrl_is_cmd_and_win_is_control() {
        let mut m = Mapper::new(Dir::PcToMac);
        assert_eq!(m.map(K::ControlLeft, true, ""), keys(&[(K::MetaLeft, true)]));
        for (k, ch) in [(K::KeyC, "c"), (K::KeyV, "v"), (K::KeyX, "x"), (K::KeyA, "a"), (K::KeyZ, "z"), (K::KeyS, "s"), (K::KeyF, "f"), (K::KeyW, "w"), (K::KeyT, "t")] {
            assert_eq!(m.map(k, true, ch), keys(&[(k, true)]));
            assert_eq!(m.map(k, false, ""), keys(&[(k, false)]));
        }
        assert_eq!(m.map(K::ControlLeft, false, ""), keys(&[(K::MetaLeft, false)]));
        assert_eq!(m.map(K::MetaLeft, true, ""), keys(&[(K::ControlLeft, true)]));
        assert_eq!(m.map(K::MetaLeft, false, ""), keys(&[(K::ControlLeft, false)]));
        assert_eq!(m.map(K::Alt, true, ""), keys(&[(K::Alt, true)])); // Alt = Option
        // toggle off: Ctrl stays Control (terminals)
        let mut m = Mapper::new(Dir::PcToMac);
        m.shortcuts = false;
        assert_eq!(m.map(K::ControlLeft, true, ""), keys(&[(K::ControlLeft, true)]));
        assert_eq!(m.map(K::Home, true, ""), keys(&[(K::Home, true)]));
    }

    #[test]
    fn pc_to_mac_rules() {
        let mut m = Mapper::new(Dir::PcToMac);
        // Ctrl+Y = redo = Shift+Cmd+Z; Ctrl is pressed as Cmd again afterwards
        m.map(K::ControlLeft, true, "");
        assert_eq!(m.map(K::KeyY, true, "y"), keys(&[(K::MetaLeft, false), (K::MetaLeft, true), (K::ShiftLeft, true), (K::KeyZ, true)]));
        assert_eq!(m.map(K::KeyY, true, "y"), keys(&[(K::KeyZ, true)])); // repeat
        assert_eq!(m.map(K::KeyY, false, ""), keys(&[(K::KeyZ, false), (K::ShiftLeft, false), (K::MetaLeft, false), (K::MetaLeft, true)]));
        // Ctrl+Left = word left = Option+Left
        assert_eq!(m.map(K::LeftArrow, true, ""), keys(&[(K::MetaLeft, false), (K::Alt, true), (K::LeftArrow, true)]));
        assert_eq!(m.map(K::LeftArrow, false, ""), keys(&[(K::LeftArrow, false), (K::Alt, false), (K::MetaLeft, true)]));
        // Ctrl+Home = document start = Cmd+Up
        assert_eq!(m.map(K::Home, true, ""), keys(&[(K::MetaLeft, false), (K::MetaLeft, true), (K::UpArrow, true)]));
        // Ctrl released while Home is held: not pressed again
        assert_eq!(m.map(K::ControlLeft, false, ""), vec![]);
        assert_eq!(m.map(K::Home, false, ""), keys(&[(K::UpArrow, false), (K::MetaLeft, false)]));
        // Home / End = line start / end, also with Shift (select)
        m.map(K::ShiftLeft, true, "");
        assert_eq!(m.map(K::End, true, ""), keys(&[(K::MetaLeft, true), (K::RightArrow, true)]));
        assert_eq!(m.map(K::End, false, ""), keys(&[(K::RightArrow, false), (K::MetaLeft, false)]));
        m.map(K::ShiftLeft, false, "");
        // Alt+F4 = Cmd+Q
        m.map(K::Alt, true, "");
        assert_eq!(m.map(K::F4, true, ""), keys(&[(K::Alt, false), (K::MetaLeft, true), (K::KeyQ, true)]));
        assert_eq!(m.map(K::F4, false, ""), keys(&[(K::KeyQ, false), (K::MetaLeft, false), (K::Alt, true)]));
        m.map(K::Alt, false, "");
    }

    #[test]
    fn alt_tab_becomes_cmd_tab_and_keeps_the_switcher_open() {
        let mut m = Mapper::new(Dir::PcToMac);
        assert_eq!(m.map(K::Alt, true, ""), keys(&[(K::Alt, true)]));
        assert_eq!(m.map(K::Tab, true, ""), keys(&[(K::Alt, false), (K::MetaLeft, true), (K::Tab, true)]));
        assert_eq!(m.map(K::Tab, false, ""), keys(&[(K::Tab, false)]));
        assert_eq!(m.map(K::Tab, true, ""), keys(&[(K::Tab, true)])); // next app, Cmd still down
        assert_eq!(m.map(K::Tab, false, ""), keys(&[(K::Tab, false)]));
        assert_eq!(m.map(K::Alt, false, ""), keys(&[(K::MetaLeft, false)]));
    }

    #[test]
    fn german_altgr_types_the_character_on_the_mac() {
        let mut m = Mapper::new(Dir::PcToMac);
        // Windows: fake left Ctrl, then right Alt
        assert_eq!(m.map(K::ControlLeft, true, ""), keys(&[(K::MetaLeft, true)]));
        assert_eq!(m.map(K::AltGr, true, ""), keys(&[(K::MetaLeft, false)]));
        for (k, ch) in [(K::KeyQ, "@"), (K::KeyE, "€"), (K::Num8, "["), (K::Num9, "]"), (K::Num7, "{"), (K::Num0, "}"), (K::Minus, "\\"), (K::IntlBackslash, "|"), (K::RightBracket, "~")] {
            assert_eq!(m.map(k, true, ch), vec![Out::Text(ch.into())], "{ch}");
            assert_eq!(m.map(k, false, ""), vec![]);
        }
        assert_eq!(m.map(K::ControlLeft, false, ""), vec![]);
        assert_eq!(m.map(K::AltGr, false, ""), vec![]);
        assert_eq!(m.map(K::KeyQ, true, "q"), keys(&[(K::KeyQ, true)]));
        // US layout: right Alt without the fake Ctrl is Option
        let mut m = Mapper::new(Dir::PcToMac);
        assert_eq!(m.map(K::AltGr, true, ""), keys(&[(K::AltGr, true)]));
        assert_eq!(m.map(K::KeyQ, true, "q"), keys(&[(K::KeyQ, true)]));
    }

    #[test]
    fn mac_to_pc_rules() {
        let mut m = Mapper::new(Dir::MacToPc);
        assert_eq!(m.map(K::MetaLeft, true, ""), keys(&[(K::ControlLeft, true)])); // Cmd = Ctrl
        assert_eq!(m.map(K::KeyC, true, "c"), keys(&[(K::KeyC, true)]));
        m.map(K::KeyC, false, "");
        assert_eq!(m.map(K::LeftArrow, true, ""), keys(&[(K::ControlLeft, false), (K::Home, true)]));
        assert_eq!(m.map(K::LeftArrow, false, ""), keys(&[(K::Home, false), (K::ControlLeft, true)]));
        assert_eq!(m.map(K::KeyQ, true, "q"), keys(&[(K::ControlLeft, false), (K::Alt, true), (K::F4, true)]));
        m.map(K::KeyQ, false, "");
        assert_eq!(m.map(K::Tab, true, ""), keys(&[(K::ControlLeft, false), (K::Alt, true), (K::Tab, true)]));
        m.map(K::Tab, false, "");
        assert_eq!(m.map(K::MetaLeft, false, ""), keys(&[(K::Alt, false)]));
        assert_eq!(m.map(K::ControlLeft, true, ""), keys(&[(K::MetaLeft, true)])); // Control = Win
        m.map(K::ControlLeft, false, "");
        // Option+Left = word left = Ctrl+Left; Option+L on a German Mac = @ typed as text
        m.map(K::Alt, true, "");
        assert_eq!(m.map(K::LeftArrow, true, ""), keys(&[(K::Alt, false), (K::ControlLeft, true), (K::LeftArrow, true)]));
        m.map(K::LeftArrow, false, "");
        assert_eq!(m.map(K::KeyL, true, "@"), vec![Out::Key(K::Alt, false), Out::Text("@".into())]);
        assert_eq!(m.map(K::KeyL, false, ""), keys(&[(K::Alt, true)]));
        assert_eq!(m.map(K::KeyF, true, "ƒ"), keys(&[(K::KeyF, true)])); // Alt+F stays Alt+F
    }

    #[test]
    fn release_all_lifts_everything_once() {
        let mut m = Mapper::new(Dir::PcToMac);
        m.map(K::ControlLeft, true, "");
        m.map(K::ShiftLeft, true, "");
        m.map(K::KeyA, true, "a");
        let r = m.release_all();
        assert_eq!(r[0], Out::Key(K::KeyA, false)); // non-modifiers first
        assert!(r.contains(&Out::Key(K::MetaLeft, false)) && r.contains(&Out::Key(K::ShiftLeft, false)));
        assert_eq!(r.len(), 3);
        assert_eq!(m.release_all(), vec![]);
        // a late key-up after the release sends nothing, and the next press works again
        assert_eq!(m.map(K::ControlLeft, false, ""), vec![]);
        assert_eq!(m.map(K::ControlLeft, true, ""), keys(&[(K::MetaLeft, true)]));
    }

    #[test]
    fn hook_state_machine() {
        let mut st = State::default();
        assert_eq!(st.hook_target(), None);
        st.sess("a", "Windows");
        st.sess("b", "Mac OS");
        assert_eq!(st.hook_target(), None); // nothing focused
        st.focused = Some("a".into());
        let on = |st: &State, k: &str| if cfg!(windows) { st.hook_target() == Some(k) } else { st.hook_target().is_none() };
        assert!(on(&st, "a"));
        st.sessions.get_mut("a").unwrap().paused = true; // chat field
        assert_eq!(st.hook_target(), None);
        st.sessions.get_mut("a").unwrap().paused = false;
        st.sessions.get_mut("a").unwrap().released = true; // escape / local Alt+Tab
        assert_eq!(st.hook_target(), None);
        st.sessions.get_mut("a").unwrap().released = false;
        st.sessions.get_mut("a").unwrap().system_keys = false; // toolbar toggle
        assert_eq!(st.hook_target(), None);
        st.focused = Some("b".into());
        assert!(on(&st, "b"));
        st.sessions.remove("b"); // window closed
        assert_eq!(st.hook_target(), None);
    }

    #[test]
    fn hook_actions() {
        let mut m = Mapper::new(Dir::Same);
        assert_eq!(hook_action(&m, K::Space, true, false), HookAction::Forward { pass: false });
        assert_eq!(hook_action(&m, K::Space, false, false), HookAction::Forward { pass: true });
        assert_eq!(hook_action(&m, K::CapsLock, true, false), HookAction::Forward { pass: true });
        assert_eq!(hook_action(&m, K::MetaLeft, true, false), HookAction::Forward { pass: false });
        m.map(K::Alt, true, "");
        assert_eq!(hook_action(&m, K::Space, true, false), HookAction::Forward { pass: false }); // Alt+Space
        assert_eq!(hook_action(&m, K::Tab, true, false), HookAction::LocalAltTab);
        assert_eq!(hook_action(&m, K::Tab, true, true), HookAction::Forward { pass: false });
        m.map(K::ControlLeft, true, "");
        assert_eq!(hook_action(&m, K::Home, true, true), HookAction::Escape);
    }
}
