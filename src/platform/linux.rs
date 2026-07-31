//! Linux/X11 backend: XTest keyboard synthesis and EWMH window behavior.
//!
//! The FFI below is hand-rolled and covers only the calls Promplet makes,
//! mirroring the raw bindings used on Windows and macOS. It links against
//! libX11 and libXtst and requires an X11 session; under Wayland it can type
//! only into XWayland windows.

use std::{
    ffi::{CString, c_char, c_int, c_long, c_uint, c_ulong, c_void},
    fs, io,
    os::fd::AsRawFd,
    path::Path,
    process::Command,
    ptr,
    sync::atomic::{AtomicUsize, Ordering},
    thread,
    time::Duration,
};

use fltk::{
    app,
    menu::{MenuFlag, MenuItem},
    prelude::{WidgetExt, WindowExt},
    window::DoubleWindow,
};

use super::{
    StripMenuAction,
    geometry::{Bounds, editor_position, visible_position},
    text::{TextUnit, plan_text},
};
use crate::model::Orientation;

type XDisplay = c_void;
type XWindow = c_ulong;
type Atom = c_ulong;
type KeySym = c_ulong;
type XBool = c_int;

const CLIENT_MESSAGE: c_int = 33;
const SUBSTRUCTURE_NOTIFY_MASK: c_long = 1 << 19;
const SUBSTRUCTURE_REDIRECT_MASK: c_long = 1 << 20;
const NET_WM_STATE_ADD: c_long = 1;
const INPUT_HINT: c_long = 1 << 0;
const KEYSYM_RETURN: KeySym = 0xFF0D;
const KEYSYM_TAB: KeySym = 0xFF09;

// Destinations translate a key event with whatever keyboard mapping they
// have absorbed by the time they process it, not the one in force when the
// event was posted. Typing therefore pauses after applying a batch of
// mappings, and again before those keycodes are remapped, instead of racing
// the destination. Key events themselves are also paced so slow destinations
// do not drop characters.
const MAPPING_SETTLE_DELAY: Duration = Duration::from_millis(25);
const INTER_KEY_DELAY: Duration = Duration::from_micros(1200);

const LOCK_EX: c_int = 2;
const LOCK_NB: c_int = 4;
const EWOULDBLOCK: i32 = 11;

#[link(name = "X11")]
unsafe extern "C" {
    fn XOpenDisplay(name: *const c_char) -> *mut XDisplay;
    fn XDefaultRootWindow(display: *mut XDisplay) -> XWindow;
    fn XInternAtom(display: *mut XDisplay, name: *const c_char, only_if_exists: XBool) -> Atom;
    fn XSendEvent(
        display: *mut XDisplay,
        window: XWindow,
        propagate: XBool,
        event_mask: c_long,
        event: *mut XClientMessageEvent,
    ) -> c_int;
    fn XSync(display: *mut XDisplay, discard: XBool) -> c_int;
    fn XFlush(display: *mut XDisplay) -> c_int;
    fn XGetInputFocus(display: *mut XDisplay, focus: *mut XWindow, revert_to: *mut c_int)
    -> c_int;
    fn XFree(data: *mut c_void) -> c_int;
    fn XGetWMHints(display: *mut XDisplay, window: XWindow) -> *mut XWMHints;
    fn XSetWMHints(display: *mut XDisplay, window: XWindow, hints: *const XWMHints) -> c_int;
    fn XDisplayKeycodes(
        display: *mut XDisplay,
        min_keycode: *mut c_int,
        max_keycode: *mut c_int,
    ) -> c_int;
    fn XGetKeyboardMapping(
        display: *mut XDisplay,
        first_keycode: u8,
        keycode_count: c_int,
        keysyms_per_keycode: *mut c_int,
    ) -> *mut KeySym;
    fn XChangeKeyboardMapping(
        display: *mut XDisplay,
        first_keycode: c_int,
        keysyms_per_keycode: c_int,
        keysyms: *const KeySym,
        keycode_count: c_int,
    ) -> c_int;
}

#[link(name = "Xtst")]
unsafe extern "C" {
    fn XTestFakeKeyEvent(
        display: *mut XDisplay,
        keycode: c_uint,
        is_press: XBool,
        delay: c_ulong,
    ) -> c_int;
}

unsafe extern "C" {
    fn flock(fd: c_int, operation: c_int) -> c_int;
}

#[repr(C)]
struct XWMHints {
    flags: c_long,
    input: XBool,
    initial_state: c_int,
    icon_pixmap: c_ulong,
    icon_window: XWindow,
    icon_x: c_int,
    icon_y: c_int,
    icon_mask: c_ulong,
    window_group: XWindow,
}

/// The ClientMessage member of XEvent, padded to the union's full 24-long
/// size so Xlib may copy a whole XEvent from it.
#[repr(C)]
struct XClientMessageEvent {
    r#type: c_int,
    serial: c_ulong,
    send_event: XBool,
    display: *mut XDisplay,
    window: XWindow,
    message_type: Atom,
    format: c_int,
    data: [c_long; 5],
    pad: [c_long; 12],
}

// One display connection for the process, opened on first use and kept for
// the process lifetime. Every backend call runs on the FLTK main thread.
static DISPLAY: AtomicUsize = AtomicUsize::new(0);

fn display() -> Result<*mut XDisplay, String> {
    let cached = DISPLAY.load(Ordering::Relaxed);
    if cached != 0 {
        return Ok(cached as *mut XDisplay);
    }

    // SAFETY: XOpenDisplay with a null name reads only the DISPLAY variable.
    let display = unsafe { XOpenDisplay(ptr::null()) };
    if display.is_null() {
        return Err(
            "Promplet could not connect to the X display. An X11 session is required.".to_owned(),
        );
    }
    DISPLAY.store(display as usize, Ordering::Relaxed);
    Ok(display)
}

fn atom(display: *mut XDisplay, name: &str) -> Atom {
    let name = CString::new(name).expect("atom names contain no NUL bytes");
    // SAFETY: `name` is a live NUL-terminated string; the display is open.
    unsafe { XInternAtom(display, name.as_ptr(), 0) }
}

fn strip_window_id(window: &DoubleWindow) -> Result<XWindow, String> {
    let xid = window.raw_handle() as XWindow;
    if xid == 0 {
        return Err("FLTK did not provide a native window handle.".to_owned());
    }
    Ok(xid)
}

/// Asks the window manager to change two EWMH states of a mapped window.
fn send_net_wm_state(
    display: *mut XDisplay,
    window: XWindow,
    action: c_long,
    first: &str,
    second: &str,
) -> Result<(), String> {
    let mut event = XClientMessageEvent {
        r#type: CLIENT_MESSAGE,
        serial: 0,
        send_event: 1,
        display,
        window,
        message_type: atom(display, "_NET_WM_STATE"),
        format: 32,
        data: [
            action,
            atom(display, first) as c_long,
            atom(display, second) as c_long,
            1, // source indication: a normal application
            0,
        ],
        pad: [0; 12],
    };

    // SAFETY: The event is a fully initialized, XEvent-sized structure, and
    // root-window client messages are how EWMH states are requested.
    let sent = unsafe {
        XSendEvent(
            display,
            XDefaultRootWindow(display),
            0,
            SUBSTRUCTURE_REDIRECT_MASK | SUBSTRUCTURE_NOTIFY_MASK,
            &mut event,
        )
    };
    if sent == 0 {
        return Err(format!("X11 rejected the {first} window state request."));
    }
    Ok(())
}

pub struct SingleInstanceGuard {
    _lock_file: fs::File,
}

pub fn claim_single_instance() -> Result<Option<SingleInstanceGuard>, String> {
    // The settings directory outlives temporary directories, whose periodic
    // cleanup could unlink a held lock and let a second instance start.
    let lock_directory = crate::store::settings_directory()?;
    fs::create_dir_all(&lock_directory)
        .map_err(|error| format!("Could not create {}: {error}", lock_directory.display()))?;

    let lock_path = lock_directory.join("promplet.lock");
    let lock_file = fs::File::create(&lock_path)
        .map_err(|error| format!("Could not create {}: {error}", lock_path.display()))?;

    // SAFETY: The descriptor belongs to `lock_file`, which outlives the guard;
    // flock only manipulates its advisory lock.
    if unsafe { flock(lock_file.as_raw_fd(), LOCK_EX | LOCK_NB) } == 0 {
        return Ok(Some(SingleInstanceGuard {
            _lock_file: lock_file,
        }));
    }

    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(EWOULDBLOCK) {
        // The first instance's strip is already visible on every workspace, so
        // there is nothing to raise; quietly yield to it.
        Ok(None)
    } else {
        Err(format!(
            "Linux could not check the single-instance lock: {error}"
        ))
    }
}

pub fn configure_strip_window(window: &DoubleWindow) -> Result<(), String> {
    let xid = strip_window_id(window)?;
    let display = display()?;

    // Clicking the strip must never take focus from the destination window.
    // With the input hint off and no WM_TAKE_FOCUS protocol, the window
    // follows the X "No Input" focus model and the window manager leaves
    // keyboard focus where it is.
    // SAFETY: `xid` is the live strip window; hints returned by XGetWMHints
    // are freed with XFree after XSetWMHints copies them.
    unsafe {
        let hints = XGetWMHints(display, xid);
        if hints.is_null() {
            let hints = XWMHints {
                flags: INPUT_HINT,
                input: 0,
                initial_state: 0,
                icon_pixmap: 0,
                icon_window: 0,
                icon_x: 0,
                icon_y: 0,
                icon_mask: 0,
                window_group: 0,
            };
            XSetWMHints(display, xid, &hints);
        } else {
            (*hints).flags |= INPUT_HINT;
            (*hints).input = 0;
            XSetWMHints(display, xid, hints);
            XFree(hints.cast());
        }
    }

    // Above normal windows and the panel, present on every workspace, and
    // absent from the taskbar and pager. Fullscreen windows live in a higher
    // layer, so the strip yields while another app is truly full screen.
    send_net_wm_state(
        display,
        xid,
        NET_WM_STATE_ADD,
        "_NET_WM_STATE_ABOVE",
        "_NET_WM_STATE_STICKY",
    )?;
    send_net_wm_state(
        display,
        xid,
        NET_WM_STATE_ADD,
        "_NET_WM_STATE_SKIP_TASKBAR",
        "_NET_WM_STATE_SKIP_PAGER",
    )?;

    // SAFETY: The display connection is open.
    unsafe {
        XFlush(display);
    }
    Ok(())
}

pub fn maintain_strip_z_order(_window: &DoubleWindow) -> Result<(), String> {
    // The EWMH states applied in configure_strip_window are persistent window
    // manager state, so there is no z-order to maintain by hand.
    Ok(())
}

pub fn position_bottom_right(window: &DoubleWindow, margin: i32) -> Result<(), String> {
    let margin = margin.max(0);
    let (work_x, work_y, work_w, work_h) =
        app::screen_work_area(app::screen_num(window.x(), window.y()));

    let x = work_x + work_w - window.width() - margin;
    let y = work_y + work_h - window.height() - margin;
    window.clone().set_pos(x, y);
    Ok(())
}

pub fn clamp_to_work_area(window: &DoubleWindow, margin: i32) -> Result<(i32, i32), String> {
    let work_area = work_area_bounds(window.x(), window.y());
    let strip =
        Bounds::from_position_and_size(window.x(), window.y(), window.width(), window.height());

    let (x, y) = visible_position(&work_area, &strip, margin);
    window.clone().set_pos(x, y);
    Ok((x, y))
}

pub fn position_editor(
    window: &DoubleWindow,
    anchor: &DoubleWindow,
    gap: i32,
    orientation: Orientation,
) -> Result<(), String> {
    let work_area = work_area_bounds(anchor.x(), anchor.y());
    let anchor_bounds =
        Bounds::from_position_and_size(anchor.x(), anchor.y(), anchor.width(), anchor.height());

    let (x, y) = editor_position(
        &work_area,
        &anchor_bounds,
        window.width(),
        window.height(),
        gap,
        orientation,
    );
    window.clone().set_pos(x, y);
    Ok(())
}

fn work_area_bounds(x: i32, y: i32) -> Bounds {
    let (work_x, work_y, work_w, work_h) = app::screen_work_area(app::screen_num(x, y));
    Bounds::from_position_and_size(work_x, work_y, work_w, work_h)
}

pub fn activate_window(window: &DoubleWindow) -> Result<(), String> {
    let xid = strip_window_id(window)?;
    let display = display()?;

    // Ask the window manager to focus and raise the editor as if the user had
    // activated it.
    let mut event = XClientMessageEvent {
        r#type: CLIENT_MESSAGE,
        serial: 0,
        send_event: 1,
        display,
        window: xid,
        message_type: atom(display, "_NET_ACTIVE_WINDOW"),
        format: 32,
        data: [1, 0, 0, 0, 0],
        pad: [0; 12],
    };

    // SAFETY: The event is a fully initialized, XEvent-sized structure.
    let sent = unsafe {
        XSendEvent(
            display,
            XDefaultRootWindow(display),
            0,
            SUBSTRUCTURE_REDIRECT_MASK | SUBSTRUCTURE_NOTIFY_MASK,
            &mut event,
        )
    };
    if sent == 0 {
        return Err("X11 rejected the editor activation request.".to_owned());
    }

    // SAFETY: The display connection is open.
    unsafe {
        XFlush(display);
    }
    Ok(())
}

pub fn release_activation() {
    // The window manager hands focus back to the previous window when the
    // editor unmaps; there is nothing to release by hand.
}

pub fn show_strip_menu(
    window: &DoubleWindow,
    x: i32,
    y: i32,
    orientation: Orientation,
) -> Result<Option<StripMenuAction>, String> {
    const ENTRIES: [(&str, StripMenuAction); 5] = [
        ("New Promplet…", StripMenuAction::Create),
        ("Vertical", StripMenuAction::ToggleOrientation),
        ("Show Config File", StripMenuAction::ShowConfig),
        ("Reload Config", StripMenuAction::ReloadConfig),
        ("Quit Promplet", StripMenuAction::Quit),
    ];

    let labels: Vec<&str> = ENTRIES.iter().map(|(label, _)| label).copied().collect();
    let menu = MenuItem::new(&labels);
    if orientation == Orientation::Vertical
        && let Some(mut vertical) = menu.at(1)
    {
        vertical.set_flag(MenuFlag::Toggle | MenuFlag::Value);
    }
    if let Some(mut reload) = menu.at(3) {
        reload.set_flag(MenuFlag::MenuDivider);
    }

    // FLTK popup menus take coordinates relative to the window handling the
    // event, while the caller passes screen coordinates.
    let picked = menu.popup(x - window.x(), y - window.y());
    Ok(picked
        .and_then(|item| item.label())
        .and_then(|label| {
            ENTRIES
                .iter()
                .find(|(entry_label, _)| *entry_label == label)
        })
        .map(|(_, action)| *action))
}

pub fn reveal_file(path: &Path) -> Result<(), String> {
    if !path.is_file() {
        return Err(format!(
            "The config file does not exist at {}.",
            path.display()
        ));
    }

    // There is no cross-desktop "select this file" verb; open its folder in
    // the file manager.
    let directory = path
        .parent()
        .ok_or_else(|| "The config file has no parent directory.".to_owned())?;
    Command::new("xdg-open").arg(directory).spawn().map_err(|error| {
        format!(
            "Linux could not open {} in a file manager: {error}",
            directory.display()
        )
    })?;
    Ok(())
}

pub fn insert_text(text: &str) -> Result<(), String> {
    if text.is_empty() {
        return Ok(());
    }

    let display = display()?;
    ensure_destination(display)?;

    let pool = KeycodePool::claim(display)?;
    pool.type_keysyms(&keysym_sequence(text))
}

/// The prompt text as the sequence of keysyms to press. plan_text yields
/// UTF-16 units for the Windows and macOS APIs; X11 keysyms carry whole
/// codepoints, so surrogate pairs are recombined here.
fn keysym_sequence(text: &str) -> Vec<KeySym> {
    let mut keysyms = Vec::new();
    let mut units = plan_text(text).into_iter();
    while let Some(unit) = units.next() {
        keysyms.push(match unit {
            TextUnit::Return => KEYSYM_RETURN,
            TextUnit::Unicode(high @ 0xD800..0xDC00) => {
                let Some(TextUnit::Unicode(low @ 0xDC00..0xE000)) = units.next() else {
                    continue; // plan_text never emits a lone surrogate
                };
                let high = c_ulong::from(high - 0xD800);
                let low = c_ulong::from(low - 0xDC00);
                unicode_keysym(0x10000 + (high << 10) + low)
            }
            TextUnit::Unicode(unit) => unicode_keysym(c_ulong::from(unit)),
        });
    }
    keysyms
}

fn unicode_keysym(codepoint: c_ulong) -> KeySym {
    // Latin-1 codepoints are their own keysyms; the rest use the X protocol's
    // 0x01000000 | codepoint Unicode range. Tab has no printable codepoint
    // keysym, so it becomes the function keysym destinations understand.
    match codepoint {
        0x09 => KEYSYM_TAB,
        0x20..=0xFF => codepoint,
        codepoint => 0x0100_0000 | codepoint,
    }
}

fn ensure_destination(display: *mut XDisplay) -> Result<(), String> {
    // A non-activating strip never holds focus itself, so the focused window
    // is the destination. Refuse to type into nothing, or into Promplet's own
    // editor.
    let mut focus: XWindow = 0;
    let mut revert_to: c_int = 0;
    // SAFETY: Both output pointers refer to initialized locals.
    unsafe {
        XGetInputFocus(display, &mut focus, &mut revert_to);
    }

    match focus {
        // None or PointerRoot: no window holds focus.
        0 | 1 => Err("There is no active destination window.".to_owned()),
        focus if is_own_window(focus) => {
            Err("Promplet is the active app. Click into a destination text field first.".to_owned())
        }
        _ => Ok(()),
    }
}

/// Whether the focused window belongs to Promplet itself — in practice the
/// editor, since the strip never takes focus.
fn is_own_window(focus: XWindow) -> bool {
    app::windows().is_some_and(|windows| {
        windows
            .iter()
            .any(|window| window.raw_handle() as XWindow == focus)
    })
}

/// Every keycode with no keysyms, borrowed from the keyboard map to type
/// arbitrary characters. Text is typed in batches: the batch's distinct
/// keysyms are mapped onto the pool at once, destinations get a moment to
/// absorb the new mapping, and only then are the key events sent. Each keysym
/// occupies both shift positions of its keycode, so held or locked modifiers
/// cannot change what is typed. Dropping the pool clears the mappings.
struct KeycodePool {
    display: *mut XDisplay,
    keycodes: Vec<c_int>,
}

impl KeycodePool {
    fn claim(display: *mut XDisplay) -> Result<Self, String> {
        let mut min_keycode: c_int = 0;
        let mut max_keycode: c_int = 0;
        // SAFETY: Both output pointers refer to initialized locals.
        unsafe {
            XDisplayKeycodes(display, &mut min_keycode, &mut max_keycode);
        }

        let mut keysyms_per_keycode: c_int = 0;
        // SAFETY: The keycode range is the one the server just reported; the
        // returned mapping is freed after the scan.
        let keycodes: Vec<c_int> = unsafe {
            let mapping = XGetKeyboardMapping(
                display,
                min_keycode as u8,
                max_keycode - min_keycode + 1,
                &mut keysyms_per_keycode,
            );
            if mapping.is_null() {
                return Err("X11 could not read the keyboard mapping.".to_owned());
            }

            let keycodes = (min_keycode..=max_keycode)
                .filter(|keycode| {
                    let offset = ((keycode - min_keycode) * keysyms_per_keycode) as usize;
                    (0..keysyms_per_keycode as usize).all(|slot| *mapping.add(offset + slot) == 0)
                })
                .collect();
            XFree(mapping.cast());
            keycodes
        };

        if keycodes.is_empty() {
            return Err("The keyboard map has no free keycode Promplet can type with.".to_owned());
        }
        Ok(Self { display, keycodes })
    }

    fn type_keysyms(&self, keysyms: &[KeySym]) -> Result<(), String> {
        let mut start = 0;
        while start < keysyms.len() {
            // Extend the batch until it would need more distinct keysyms than
            // the pool has keycodes.
            let mut batch: Vec<KeySym> = Vec::new();
            let mut end = start;
            while end < keysyms.len() {
                if !batch.contains(&keysyms[end]) {
                    if batch.len() == self.keycodes.len() {
                        break;
                    }
                    batch.push(keysyms[end]);
                }
                end += 1;
            }

            for (slot, keysym) in batch.iter().enumerate() {
                self.map_keycode(self.keycodes[slot], *keysym);
            }
            // SAFETY: The display connection is open.
            unsafe {
                XSync(self.display, 0);
            }
            thread::sleep(MAPPING_SETTLE_DELAY);

            for keysym in &keysyms[start..end] {
                let slot = batch
                    .iter()
                    .position(|batched| batched == keysym)
                    .expect("every keysym in the batch range was assigned a keycode");
                self.press(self.keycodes[slot])?;
            }
            // Let the destination translate this batch's events before its
            // keycodes are remapped for the next batch.
            // SAFETY: The display connection is open.
            unsafe {
                XSync(self.display, 0);
            }
            thread::sleep(MAPPING_SETTLE_DELAY);

            start = end;
        }
        Ok(())
    }

    fn map_keycode(&self, keycode: c_int, keysym: KeySym) {
        let keysyms = [keysym, keysym];
        // SAFETY: The keycode is within the server's keycode range and the
        // keysym array outlives the call.
        unsafe {
            XChangeKeyboardMapping(self.display, keycode, 2, keysyms.as_ptr(), 1);
        }
    }

    fn press(&self, keycode: c_int) -> Result<(), String> {
        // SAFETY: The keycode is within the server's keycode range; XFlush
        // sends the buffered events so the pacing delay is real.
        unsafe {
            if XTestFakeKeyEvent(self.display, keycode as c_uint, 1, 0) == 0
                || XTestFakeKeyEvent(self.display, keycode as c_uint, 0, 0) == 0
            {
                return Err("X11 rejected a synthesized key event.".to_owned());
            }
            XFlush(self.display);
        }
        thread::sleep(INTER_KEY_DELAY);
        Ok(())
    }
}

impl Drop for KeycodePool {
    fn drop(&mut self) {
        // Return every borrowed keycode to its original, keysym-free state.
        for keycode in &self.keycodes {
            let none = [0 as KeySym, 0];
            // SAFETY: As in map_keycode.
            unsafe {
                XChangeKeyboardMapping(self.display, *keycode, 2, none.as_ptr(), 1);
            }
        }
        // SAFETY: The display connection is open.
        unsafe {
            XFlush(self.display);
        }
    }
}
