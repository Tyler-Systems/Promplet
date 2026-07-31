//! macOS backend: Quartz keyboard synthesis and Cocoa window behavior.
//!
//! The FFI below is hand-rolled and covers only the calls Promplet makes,
//! mirroring the raw `windows-sys` bindings used on Windows. No call here
//! returns a structure by value, so casting `objc_msgSend` per signature is
//! sound on both Apple targets.

use std::{
    ffi::{CString, c_char, c_int, c_ulong, c_void},
    fs, io, mem,
    os::fd::AsRawFd,
    path::Path,
    process::Command,
    ptr,
    sync::{
        Once,
        atomic::{AtomicI32, AtomicIsize, AtomicUsize, Ordering},
    },
    thread,
    time::Duration,
};

use fltk::{
    app,
    prelude::{WidgetExt, WindowExt},
    window::DoubleWindow,
};

use super::{
    StripMenuAction,
    geometry::{Bounds, editor_position, visible_position},
    text::{TextUnit, plan_text},
};
use crate::model::Orientation;

type Id = *mut c_void;
type Sel = *mut c_void;
type CGEventRef = *mut c_void;

// NSApplicationActivationPolicyAccessory: no Dock icon or menu bar, but the
// editor can still take keyboard focus when opened.
const ACTIVATION_POLICY_ACCESSORY: isize = 1;

// NSWindowCollectionBehavior: follow the user to every Space, hold still
// during Mission Control, and stay out of the window cycle. Fullscreen
// auxiliary behavior is deliberately absent so the strip yields while another
// app is truly full screen.
const COLLECTION_CAN_JOIN_ALL_SPACES: isize = 1 << 0;
const COLLECTION_STATIONARY: isize = 1 << 4;
const COLLECTION_IGNORES_CYCLE: isize = 1 << 6;

const KEY_RETURN: u16 = 36; // kVK_Return
const HID_EVENT_TAP: u32 = 0; // kCGHIDEventTap

// CGEventKeyboardSetUnicodeString truncates longer payloads, and posting
// events back to back with no pause can drop characters in slow destinations.
const UNICODE_CHUNK: usize = 20;
const INTER_EVENT_DELAY: Duration = Duration::from_micros(1500);

const LOCK_EX: c_int = 2;
const LOCK_NB: c_int = 4;
const EWOULDBLOCK: i32 = 35;

#[link(name = "AppKit", kind = "framework")]
unsafe extern "C" {}

#[repr(C)]
#[derive(Clone, Copy)]
struct CGPoint {
    x: f64,
    y: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CGSize {
    width: f64,
    height: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CGRect {
    origin: CGPoint,
    size: CGSize,
}

#[link(name = "objc")]
unsafe extern "C" {
    fn objc_getClass(name: *const c_char) -> Id;
    fn sel_registerName(name: *const c_char) -> Sel;
    fn objc_msgSend();
    fn objc_allocateClassPair(superclass: Id, name: *const c_char, extra_bytes: usize) -> Id;
    fn objc_registerClassPair(class: Id);
    fn class_addMethod(
        class: Id,
        selector: Sel,
        implementation: unsafe extern "C" fn(Id, Sel, Id),
        types: *const c_char,
    ) -> i8;
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventCreateKeyboardEvent(
        source: *mut c_void,
        virtual_key: u16,
        key_down: bool,
    ) -> CGEventRef;
    fn CGEventKeyboardSetUnicodeString(event: CGEventRef, length: c_ulong, string: *const u16);
    fn CGEventPost(tap: u32, event: CGEventRef);
    fn CGMainDisplayID() -> u32;
    fn CGDisplayBounds(display: u32) -> CGRect;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(value: *const c_void);
    fn CFDictionaryCreate(
        allocator: *const c_void,
        keys: *const *const c_void,
        values: *const *const c_void,
        count: isize,
        key_callbacks: *const c_void,
        value_callbacks: *const c_void,
    ) -> *const c_void;
    static kCFTypeDictionaryKeyCallBacks: usize;
    static kCFTypeDictionaryValueCallBacks: usize;
    static kCFBooleanTrue: *const c_void;
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrustedWithOptions(options: *const c_void) -> bool;
    static kAXTrustedCheckOptionPrompt: *const c_void;
}

unsafe extern "C" {
    fn flock(fd: c_int, operation: c_int) -> c_int;
}

fn class(name: &str) -> Id {
    let name = CString::new(name).expect("class names contain no NUL bytes");
    // SAFETY: `name` is a live NUL-terminated string; objc_getClass only reads
    // it to look up the class.
    unsafe { objc_getClass(name.as_ptr()) }
}

fn selector(name: &str) -> Sel {
    let name = CString::new(name).expect("selector names contain no NUL bytes");
    // SAFETY: `name` is a live NUL-terminated string; sel_registerName interns
    // it and returns a process-lifetime selector.
    unsafe { sel_registerName(name.as_ptr()) }
}

// SAFETY contract for every msg_* helper: `receiver` is a valid Objective-C
// object (or class) and `selector` names a method on it whose C signature
// matches the helper's argument and return types exactly.

unsafe fn msg_id(receiver: Id, selector: Sel) -> Id {
    let send: unsafe extern "C" fn(Id, Sel) -> Id =
        // SAFETY: objc_msgSend dispatches with the ABI of the target method,
        // which the caller guarantees matches this signature.
        unsafe { mem::transmute(objc_msgSend as unsafe extern "C" fn()) };
    unsafe { send(receiver, selector) }
}

unsafe fn msg_void(receiver: Id, selector: Sel) {
    let send: unsafe extern "C" fn(Id, Sel) =
        // SAFETY: as in msg_id.
        unsafe { mem::transmute(objc_msgSend as unsafe extern "C" fn()) };
    unsafe { send(receiver, selector) }
}

unsafe fn msg_void_id(receiver: Id, selector: Sel, argument: Id) {
    let send: unsafe extern "C" fn(Id, Sel, Id) =
        // SAFETY: as in msg_id.
        unsafe { mem::transmute(objc_msgSend as unsafe extern "C" fn()) };
    unsafe { send(receiver, selector, argument) }
}

unsafe fn msg_void_isize(receiver: Id, selector: Sel, argument: isize) {
    let send: unsafe extern "C" fn(Id, Sel, isize) =
        // SAFETY: as in msg_id.
        unsafe { mem::transmute(objc_msgSend as unsafe extern "C" fn()) };
    unsafe { send(receiver, selector, argument) }
}

unsafe fn msg_bool_isize(receiver: Id, selector: Sel, argument: isize) -> i8 {
    let send: unsafe extern "C" fn(Id, Sel, isize) -> i8 =
        // SAFETY: as in msg_id.
        unsafe { mem::transmute(objc_msgSend as unsafe extern "C" fn()) };
    unsafe { send(receiver, selector, argument) }
}

unsafe fn msg_void_bool(receiver: Id, selector: Sel, argument: bool) {
    let send: unsafe extern "C" fn(Id, Sel, i8) =
        // SAFETY: as in msg_id. BOOL is one byte on both Apple targets.
        unsafe { mem::transmute(objc_msgSend as unsafe extern "C" fn()) };
    unsafe { send(receiver, selector, argument as i8) }
}

unsafe fn msg_bool_sel(receiver: Id, selector: Sel, argument: Sel) -> i8 {
    let send: unsafe extern "C" fn(Id, Sel, Sel) -> i8 =
        // SAFETY: as in msg_id.
        unsafe { mem::transmute(objc_msgSend as unsafe extern "C" fn()) };
    unsafe { send(receiver, selector, argument) }
}

unsafe fn msg_i32(receiver: Id, selector: Sel) -> i32 {
    let send: unsafe extern "C" fn(Id, Sel) -> i32 =
        // SAFETY: as in msg_id.
        unsafe { mem::transmute(objc_msgSend as unsafe extern "C" fn()) };
    unsafe { send(receiver, selector) }
}

unsafe fn msg_id_i32(receiver: Id, selector: Sel, argument: i32) -> Id {
    let send: unsafe extern "C" fn(Id, Sel, i32) -> Id =
        // SAFETY: as in msg_id.
        unsafe { mem::transmute(objc_msgSend as unsafe extern "C" fn()) };
    unsafe { send(receiver, selector, argument) }
}

unsafe fn msg_bool_usize(receiver: Id, selector: Sel, argument: usize) -> i8 {
    let send: unsafe extern "C" fn(Id, Sel, usize) -> i8 =
        // SAFETY: as in msg_id.
        unsafe { mem::transmute(objc_msgSend as unsafe extern "C" fn()) };
    unsafe { send(receiver, selector, argument) }
}

unsafe fn msg_isize(receiver: Id, selector: Sel) -> isize {
    let send: unsafe extern "C" fn(Id, Sel) -> isize =
        // SAFETY: as in msg_id.
        unsafe { mem::transmute(objc_msgSend as unsafe extern "C" fn()) };
    unsafe { send(receiver, selector) }
}

unsafe fn msg_id_cstr(receiver: Id, selector: Sel, argument: *const c_char) -> Id {
    let send: unsafe extern "C" fn(Id, Sel, *const c_char) -> Id =
        // SAFETY: as in msg_id.
        unsafe { mem::transmute(objc_msgSend as unsafe extern "C" fn()) };
    unsafe { send(receiver, selector, argument) }
}

unsafe fn msg_id_id(receiver: Id, selector: Sel, argument: Id) -> Id {
    let send: unsafe extern "C" fn(Id, Sel, Id) -> Id =
        // SAFETY: as in msg_id.
        unsafe { mem::transmute(objc_msgSend as unsafe extern "C" fn()) };
    unsafe { send(receiver, selector, argument) }
}

unsafe fn msg_id_id_sel_id(receiver: Id, selector: Sel, first: Id, second: Sel, third: Id) -> Id {
    let send: unsafe extern "C" fn(Id, Sel, Id, Sel, Id) -> Id =
        // SAFETY: as in msg_id.
        unsafe { mem::transmute(objc_msgSend as unsafe extern "C" fn()) };
    unsafe { send(receiver, selector, first, second, third) }
}

unsafe fn msg_bool_id_point_id(
    receiver: Id,
    selector: Sel,
    item: Id,
    location: CGPoint,
    view: Id,
) -> i8 {
    let send: unsafe extern "C" fn(Id, Sel, Id, CGPoint, Id) -> i8 =
        // SAFETY: as in msg_id. NSPoint and CGPoint share one layout, and a
        // small struct argument follows the plain C ABI on both Apple targets.
        unsafe { mem::transmute(objc_msgSend as unsafe extern "C" fn()) };
    unsafe { send(receiver, selector, item, location, view) }
}

/// An autoreleased NSString; valid at least until the current event cycle's
/// autorelease pool drains.
fn ns_string(text: &str) -> Id {
    let text = CString::new(text).expect("menu text contains no NUL bytes");
    // SAFETY: +stringWithUTF8String: copies the bytes before returning.
    unsafe {
        msg_id_cstr(
            class("NSString"),
            selector("stringWithUTF8String:"),
            text.as_ptr(),
        )
    }
}

fn shared_application() -> Id {
    // SAFETY: +sharedApplication is a class method returning the NSApplication
    // singleton; FLTK has already created it before any backend call runs.
    unsafe { msg_id(class("NSApplication"), selector("sharedApplication")) }
}

// The pid of the app that held activation before Promplet last took it, so
// release_activation can hand activation back. macOS keeps an app active
// until another one is activated, so a bare deactivate is not enough.
static PREVIOUS_FRONTMOST_PID: AtomicI32 = AtomicI32::new(0);

fn frontmost_application_pid() -> Option<i32> {
    // SAFETY: NSWorkspace and NSRunningApplication calls match their
    // documented signatures.
    unsafe {
        let workspace = msg_id(class("NSWorkspace"), selector("sharedWorkspace"));
        if workspace.is_null() {
            return None;
        }
        let frontmost = msg_id(workspace, selector("frontmostApplication"));
        if frontmost.is_null() {
            return None;
        }
        Some(msg_i32(frontmost, selector("processIdentifier")))
    }
}

fn remember_frontmost_application() {
    if let Some(pid) = frontmost_application_pid()
        && pid != std::process::id() as i32
    {
        PREVIOUS_FRONTMOST_PID.store(pid, Ordering::Relaxed);
    }
}

pub struct SingleInstanceGuard {
    _lock_file: fs::File,
}

pub fn claim_single_instance() -> Result<Option<SingleInstanceGuard>, String> {
    // Promplet is about to take activation away from the user's current app
    // (FLTK activates itself while starting); note that app so the strip can
    // hand activation back once it is up.
    remember_frontmost_application();

    // The settings directory outlives temporary directories, whose periodic
    // cleanup could unlink a held lock and let a second instance start.
    let home = std::env::var_os("HOME")
        .ok_or_else(|| "macOS did not provide a home directory.".to_owned())?;
    let lock_directory = std::path::PathBuf::from(home)
        .join("Library")
        .join("Application Support")
        .join("Promplet");
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
        // The first instance's strip is already visible on every Space, so
        // there is nothing to raise; quietly yield to it.
        Ok(None)
    } else {
        Err(format!(
            "macOS could not check the single-instance lock: {error}"
        ))
    }
}

pub fn configure_strip_window(window: &DoubleWindow) -> Result<(), String> {
    let ns_window = window.raw_handle() as Id;
    if ns_window.is_null() {
        return Err("FLTK did not provide a native window handle.".to_owned());
    }

    // SAFETY: The receivers are the live NSApplication and the strip's
    // NSWindow on the main thread, and each selector matches its documented
    // AppKit signature.
    unsafe {
        let ns_app = shared_application();
        if !ns_app.is_null() {
            // Accessory policy removes the Dock icon FLTK requested for this
            // unbundled process; the strip is the entire interface.
            let _ = msg_bool_isize(
                ns_app,
                selector("setActivationPolicy:"),
                ACTIVATION_POLICY_ACCESSORY,
            );
        }

        msg_void_isize(
            ns_window,
            selector("setCollectionBehavior:"),
            COLLECTION_CAN_JOIN_ALL_SPACES | COLLECTION_STATIONARY | COLLECTION_IGNORES_CYCLE,
        );

        // Clicking the strip must not steal focus from the destination app.
        // AppKit documents this switch only for non-activating NSPanels, but
        // this NSWindow setter has been the same mechanism for decades.
        let prevent_activation = selector("_setPreventsActivation:");
        if msg_bool_sel(
            ns_window,
            selector("respondsToSelector:"),
            prevent_activation,
        ) != 0
        {
            msg_void_bool(ns_window, prevent_activation, true);
        } else {
            eprintln!(
                "This macOS build does not support non-activating windows; clicking the strip will steal focus."
            );
        }
    }

    Ok(())
}

pub fn maintain_strip_z_order(_window: &DoubleWindow) -> Result<(), String> {
    // FLTK's set_on_top keeps the strip in a floating window level, and the
    // collection behavior applied in configure_strip_window keeps it off other
    // apps' fullscreen Spaces, so there is no z-order to maintain by hand.
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
    let ns_window = window.raw_handle() as Id;
    if ns_window.is_null() {
        return Err("FLTK did not provide a native editor window handle.".to_owned());
    }

    remember_frontmost_application();

    // SAFETY: The receivers are the live NSApplication and editor NSWindow on
    // the main thread. Activation is requested in direct response to a user
    // click, which macOS honors.
    unsafe {
        let ns_app = shared_application();
        if ns_app.is_null() {
            return Err("macOS did not provide the running application.".to_owned());
        }
        msg_void_bool(ns_app, selector("activateIgnoringOtherApps:"), true);
        msg_void_id(
            ns_window,
            selector("makeKeyAndOrderFront:"),
            ptr::null_mut(),
        );
    }

    Ok(())
}

pub fn release_activation() {
    // Hand activation back to the app it was taken from, so the next strip
    // click types into that app instead of hitting Promplet itself. macOS
    // keeps an app active until another is activated, so activate the noted
    // app rather than merely deactivating. Windows restores focus on its own
    // when the editor hides.
    if frontmost_application_pid() != Some(std::process::id() as i32) {
        return;
    }

    // SAFETY: NSRunningApplication and NSApplication calls match their
    // documented signatures; a pid that is no longer running yields nil,
    // which is checked before use.
    unsafe {
        let previous_pid = PREVIOUS_FRONTMOST_PID.load(Ordering::Relaxed);
        if previous_pid > 0 {
            let previous = msg_id_i32(
                class("NSRunningApplication"),
                selector("runningApplicationWithProcessIdentifier:"),
                previous_pid,
            );
            if !previous.is_null()
                && msg_bool_usize(previous, selector("activateWithOptions:"), 0) != 0
            {
                return;
            }
        }

        let ns_app = shared_application();
        if !ns_app.is_null() {
            msg_void(ns_app, selector("deactivate"));
        }
    }
}

// The tag of the item picked from the most recent grip menu, written by the
// menu target's action method. -1 means nothing was picked.
static PICKED_MENU_TAG: AtomicIsize = AtomicIsize::new(-1);

// A single NSObject subclass instance that receives every menu item's action
// and records the item's tag. Registered and allocated once, kept for the
// process lifetime.
static MENU_TARGET: AtomicUsize = AtomicUsize::new(0);
static MENU_TARGET_INIT: Once = Once::new();

unsafe extern "C" fn menu_item_picked(_this: Id, _command: Sel, sender: Id) {
    // SAFETY: `sender` is the NSMenuItem that fired this action; -tag returns
    // its NSInteger tag.
    let tag = unsafe { msg_isize(sender, selector("tag")) };
    PICKED_MENU_TAG.store(tag, Ordering::Relaxed);
}

fn menu_target() -> Id {
    MENU_TARGET_INIT.call_once(|| {
        let name = CString::new("PrompletMenuTarget").expect("class name contains no NUL bytes");
        let types = CString::new("v@:@").expect("type encoding contains no NUL bytes");
        // SAFETY: The class pair is registered exactly once before any
        // instance exists, and the added method's C signature matches the
        // "v@:@" encoding of menu_item_picked.
        unsafe {
            let target_class = objc_allocateClassPair(class("NSObject"), name.as_ptr(), 0);
            if target_class.is_null() {
                return;
            }
            class_addMethod(
                target_class,
                selector("menuItemPicked:"),
                menu_item_picked,
                types.as_ptr(),
            );
            objc_registerClassPair(target_class);

            let instance = msg_id(msg_id(target_class, selector("alloc")), selector("init"));
            MENU_TARGET.store(instance as usize, Ordering::Relaxed);
        }
    });
    MENU_TARGET.load(Ordering::Relaxed) as Id
}

pub fn show_strip_menu(
    _window: &DoubleWindow,
    x: i32,
    y: i32,
    orientation: Orientation,
) -> Result<Option<StripMenuAction>, String> {
    let target = menu_target();
    if target.is_null() {
        return Err("macOS could not create the grip menu target.".to_owned());
    }

    const ENTRIES: [(&str, StripMenuAction); 5] = [
        ("New Promplet…", StripMenuAction::Create),
        ("Vertical", StripMenuAction::ToggleOrientation),
        ("Show Config File", StripMenuAction::ShowConfig),
        ("Reload Config", StripMenuAction::ReloadConfig),
        ("Quit Promplet", StripMenuAction::Quit),
    ];
    PICKED_MENU_TAG.store(-1, Ordering::Relaxed);

    // SAFETY: The menu and its items are created, shown, and released on the
    // main thread within this call; every selector matches its documented
    // AppKit signature. popUpMenuPositioningItem:atLocation:inView: runs the
    // menu synchronously without activating the app.
    unsafe {
        let menu = msg_id_id(
            msg_id(class("NSMenu"), selector("alloc")),
            selector("initWithTitle:"),
            ns_string(""),
        );
        if menu.is_null() {
            return Err("macOS could not create the grip menu.".to_owned());
        }
        msg_void_bool(menu, selector("setAutoenablesItems:"), false);

        for (index, (title, action)) in ENTRIES.iter().enumerate() {
            if *action == StripMenuAction::Quit {
                let separator = msg_id(class("NSMenuItem"), selector("separatorItem"));
                msg_void_id(menu, selector("addItem:"), separator);
            }

            let item = msg_id_id_sel_id(
                msg_id(class("NSMenuItem"), selector("alloc")),
                selector("initWithTitle:action:keyEquivalent:"),
                ns_string(title),
                selector("menuItemPicked:"),
                ns_string(""),
            );
            msg_void_id(item, selector("setTarget:"), target);
            msg_void_isize(item, selector("setTag:"), index as isize);
            if *action == StripMenuAction::ToggleOrientation && orientation == Orientation::Vertical
            {
                msg_void_isize(item, selector("setState:"), 1);
            }
            msg_void_id(menu, selector("addItem:"), item);
            msg_void(item, selector("release"));
        }

        // AppKit's global coordinates put the origin at the primary display's
        // bottom-left corner, so flip the y FLTK reports.
        let location = CGPoint {
            x: f64::from(x),
            y: CGDisplayBounds(CGMainDisplayID()).size.height - f64::from(y),
        };
        msg_bool_id_point_id(
            menu,
            selector("popUpMenuPositioningItem:atLocation:inView:"),
            ptr::null_mut(),
            location,
            ptr::null_mut(),
        );
        msg_void(menu, selector("release"));
    }

    let picked = PICKED_MENU_TAG.swap(-1, Ordering::Relaxed);
    Ok(usize::try_from(picked)
        .ok()
        .and_then(|index| ENTRIES.get(index))
        .map(|(_, action)| *action))
}

pub fn reveal_file(path: &Path) -> Result<(), String> {
    if !path.is_file() {
        return Err(format!(
            "The config file does not exist at {}.",
            path.display()
        ));
    }

    Command::new("/usr/bin/open")
        .arg("-R")
        .arg(path)
        .spawn()
        .map_err(|error| {
            format!(
                "macOS could not reveal {} in Finder: {error}",
                path.display()
            )
        })?;
    Ok(())
}

pub fn insert_text(text: &str) -> Result<(), String> {
    if text.is_empty() {
        return Ok(());
    }

    ensure_destination()?;
    ensure_accessibility_permission()?;

    let mut pending: Vec<u16> = Vec::new();
    for unit in plan_text(text) {
        match unit {
            TextUnit::Unicode(code_unit) => {
                pending.push(code_unit);
                if pending.len() >= UNICODE_CHUNK {
                    // Never split a surrogate pair across events: hold a
                    // trailing high surrogate for the next chunk.
                    let ends_in_high_surrogate =
                        matches!(pending.last(), Some(&unit) if (0xD800..0xDC00).contains(&unit));
                    let split = if ends_in_high_surrogate {
                        pending.len() - 1
                    } else {
                        pending.len()
                    };
                    if split > 0 {
                        post_unicode(&pending[..split])?;
                        pending.drain(..split);
                    }
                }
            }
            TextUnit::Return => {
                flush_pending(&mut pending)?;
                press_key(KEY_RETURN)?;
            }
        }
    }
    flush_pending(&mut pending)?;

    Ok(())
}

fn flush_pending(pending: &mut Vec<u16>) -> Result<(), String> {
    if !pending.is_empty() {
        post_unicode(pending)?;
        pending.clear();
    }
    Ok(())
}

fn ensure_destination() -> Result<(), String> {
    // A non-activating strip never takes focus itself, so the frontmost app is
    // the destination. Refuse to type into nothing, or into Promplet's own
    // editor.
    match frontmost_application_pid() {
        None => Err("There is no active destination window.".to_owned()),
        Some(pid) if pid == std::process::id() as i32 => {
            Err("Promplet is the active app. Click into a destination text field first.".to_owned())
        }
        Some(_) => Ok(()),
    }
}

fn ensure_accessibility_permission() -> Result<(), String> {
    // SAFETY: The dictionary pairs one constant key with one constant value
    // using the standard type callbacks; AXIsProcessTrustedWithOptions only
    // reads it. Asking with the prompt option makes macOS show its one-time
    // permission dialog.
    let trusted = unsafe {
        let keys = [kAXTrustedCheckOptionPrompt];
        let values = [kCFBooleanTrue];
        let options = CFDictionaryCreate(
            ptr::null(),
            keys.as_ptr(),
            values.as_ptr(),
            1,
            (&raw const kCFTypeDictionaryKeyCallBacks).cast(),
            (&raw const kCFTypeDictionaryValueCallBacks).cast(),
        );
        let trusted = AXIsProcessTrustedWithOptions(options);
        if !options.is_null() {
            CFRelease(options);
        }
        trusted
    };

    if trusted {
        Ok(())
    } else {
        Err("macOS needs permission before Promplet can type.\n\nEnable Promplet (or the terminal that launched it) under System Settings → Privacy & Security → Accessibility, then click the prompt again. Relaunch Promplet if it still cannot type.".to_owned())
    }
}

fn post_unicode(units: &[u16]) -> Result<(), String> {
    debug_assert!(!units.is_empty() && units.len() <= UNICODE_CHUNK);

    for key_down in [true, false] {
        // SAFETY: The event is created, given a payload that lives across the
        // call, posted, and released synchronously.
        unsafe {
            let event = CGEventCreateKeyboardEvent(ptr::null_mut(), 0, key_down);
            if event.is_null() {
                return Err("macOS could not create a keyboard event.".to_owned());
            }
            CGEventKeyboardSetUnicodeString(event, units.len() as c_ulong, units.as_ptr());
            CGEventPost(HID_EVENT_TAP, event);
            CFRelease(event);
        }
    }
    thread::sleep(INTER_EVENT_DELAY);
    Ok(())
}

fn press_key(virtual_key: u16) -> Result<(), String> {
    for key_down in [true, false] {
        // SAFETY: The event is created, posted, and released synchronously.
        unsafe {
            let event = CGEventCreateKeyboardEvent(ptr::null_mut(), virtual_key, key_down);
            if event.is_null() {
                return Err("macOS could not create a keyboard event.".to_owned());
            }
            CGEventPost(HID_EVENT_TAP, event);
            CFRelease(event);
        }
    }
    thread::sleep(INTER_EVENT_DELAY);
    Ok(())
}
