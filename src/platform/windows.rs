use std::{ffi::OsString, io, mem, path::Path, process::Command, ptr, thread, time::Duration};

use fltk::{prelude::WindowExt, window::DoubleWindow};
use windows_sys::Win32::{
    Foundation::{
        CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, HWND, RECT, SetLastError,
    },
    Graphics::Gdi::{
        GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTOPRIMARY, MONITORINFO,
        MonitorFromWindow,
    },
    System::Threading::CreateMutexW,
    UI::{
        Input::KeyboardAndMouse::{
            INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE,
            SendInput, VK_RETURN,
        },
        WindowsAndMessaging::{
            AppendMenuW, CreatePopupMenu, DestroyMenu, FindWindowExW, FindWindowW, GW_HWNDPREV,
            GWL_EXSTYLE, GetClassNameW, GetForegroundWindow, GetWindow, GetWindowLongPtrW,
            GetWindowRect, HWND_TOPMOST, MF_SEPARATOR, MF_STRING, PostMessageW, SWP_FRAMECHANGED,
            SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, SetForegroundWindow,
            SetWindowLongPtrW, SetWindowPos, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenuEx,
            WM_NULL, WS_EX_NOACTIVATE,
        },
    },
};

const INPUT_CHUNK_SIZE: usize = 256;
const INSTANCE_MUTEX_NAME: &str = r"Local\Promplet.SingleInstance.v1";
const STRIP_WINDOW_TITLE: &str = "Promplet";
const EXISTING_WINDOW_RETRIES: usize = 20;
const EXISTING_WINDOW_RETRY_DELAY: Duration = Duration::from_millis(50);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TextUnit {
    Return,
    Unicode(u16),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StripMenuAction {
    Create,
    ShowConfig,
    Quit,
}

pub struct SingleInstanceGuard {
    mutex: HANDLE,
}

impl Drop for SingleInstanceGuard {
    fn drop(&mut self) {
        // SAFETY: `mutex` is the live handle returned by `CreateMutexW`, and
        // this guard owns it for the lifetime of the primary process.
        unsafe {
            CloseHandle(self.mutex);
        }
    }
}

pub fn claim_single_instance() -> Result<Option<SingleInstanceGuard>, String> {
    let mutex_name = wide_null(INSTANCE_MUTEX_NAME);

    // SAFETY: The security attributes pointer is null, the name is
    // NUL-terminated, and the returned handle is either closed below or owned
    // by `SingleInstanceGuard`.
    let (mutex, already_running) = unsafe {
        SetLastError(0);
        let mutex = CreateMutexW(ptr::null(), 0, mutex_name.as_ptr());
        if mutex.is_null() {
            return Err(format!(
                "Windows could not create the single-instance guard: {}",
                io::Error::last_os_error()
            ));
        }
        (mutex, GetLastError() == ERROR_ALREADY_EXISTS)
    };

    if already_running {
        // SAFETY: This process does not own the existing named mutex; it only
        // closes the handle returned by its own `CreateMutexW` call.
        unsafe {
            CloseHandle(mutex);
        }
        wake_existing_instance()?;
        Ok(None)
    } else {
        Ok(Some(SingleInstanceGuard { mutex }))
    }
}

fn wake_existing_instance() -> Result<(), String> {
    let window_title = wide_null(STRIP_WINDOW_TITLE);

    for attempt in 0..EXISTING_WINDOW_RETRIES {
        // SAFETY: The class pointer is null and `window_title` is a live,
        // NUL-terminated UTF-16 buffer.
        let hwnd = unsafe { FindWindowW(ptr::null(), window_title.as_ptr()) };
        if !hwnd.is_null() {
            // SAFETY: `hwnd` is the top-level strip window returned by
            // `FindWindowW`. The call shows it and raises it without stealing
            // focus from the user's current text field.
            if unsafe {
                SetWindowPos(
                    hwnd,
                    HWND_TOPMOST,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
                )
            } == 0
            {
                return Err(format!(
                    "Windows could not raise the running Promplet strip: {}",
                    io::Error::last_os_error()
                ));
            }
            return Ok(());
        }

        if attempt + 1 < EXISTING_WINDOW_RETRIES {
            thread::sleep(EXISTING_WINDOW_RETRY_DELAY);
        }
    }

    Err("Another Promplet process is running, but Windows could not find its strip.".to_owned())
}

pub fn configure_strip_window(window: &DoubleWindow) -> Result<(), String> {
    let hwnd = window.raw_handle() as HWND;
    if hwnd.is_null() {
        return Err("FLTK did not provide a native window handle.".to_owned());
    }

    // SAFETY: `hwnd` belongs to the live FLTK window on this UI thread. The
    // calls only update documented window styles and z-order flags.
    unsafe {
        let current_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        let desired_style = current_style | WS_EX_NOACTIVATE;
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, desired_style as isize);

        let positioned = SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        );
        if positioned == 0 {
            return Err(format!(
                "Windows could not configure the strip: {}",
                io::Error::last_os_error()
            ));
        }
    }

    Ok(())
}

pub fn maintain_strip_z_order(window: &DoubleWindow) -> Result<(), String> {
    let hwnd = window.raw_handle() as HWND;
    if hwnd.is_null() {
        return Err("FLTK did not provide a native strip window handle.".to_owned());
    }

    if foreground_window_is_fullscreen(hwnd)? {
        return Ok(());
    }

    let mut strip_rect = RECT::default();
    if unsafe { GetWindowRect(hwnd, &mut strip_rect) } == 0 {
        return Err(format!(
            "Windows could not inspect the strip position: {}",
            io::Error::last_os_error()
        ));
    }

    let Some(taskbar) = overlapping_taskbar(&strip_rect)? else {
        return Ok(());
    };
    if window_is_above(taskbar, hwnd) {
        raise_strip_window(window)?;
    }

    Ok(())
}

fn raise_strip_window(window: &DoubleWindow) -> Result<(), String> {
    let hwnd = window.raw_handle() as HWND;

    // SAFETY: `hwnd` belongs to the live FLTK strip window. This only moves it
    // to the front of Windows' topmost band without moving, sizing, or
    // activating it.
    if unsafe {
        SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        )
    } == 0
    {
        return Err(format!(
            "Windows could not keep the strip above the taskbar: {}",
            io::Error::last_os_error()
        ));
    }

    Ok(())
}

pub fn position_bottom_right(window: &DoubleWindow, margin: i32) -> Result<(), String> {
    let hwnd = window.raw_handle() as HWND;
    if hwnd.is_null() {
        return Err("FLTK did not provide a native strip window handle.".to_owned());
    }

    let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTOPRIMARY) };
    if monitor.is_null() {
        return Err("Windows could not identify the strip's monitor.".to_owned());
    }

    let mut monitor_info = MONITORINFO {
        cbSize: mem::size_of::<MONITORINFO>() as u32,
        ..MONITORINFO::default()
    };
    let mut window_rect = RECT::default();

    // SAFETY: The monitor and window handles are live and both output pointers
    // refer to initialized, correctly sized structures.
    unsafe {
        if GetMonitorInfoW(monitor, &mut monitor_info) == 0
            || GetWindowRect(hwnd, &mut window_rect) == 0
        {
            return Err(format!(
                "Windows could not read the desktop work area: {}",
                io::Error::last_os_error()
            ));
        }

        let width = window_rect.right - window_rect.left;
        let height = window_rect.bottom - window_rect.top;
        let x = monitor_info.rcWork.right - width - margin.max(0);
        let y = monitor_info.rcWork.bottom - height - margin.max(0);

        if SetWindowPos(hwnd, HWND_TOPMOST, x, y, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE) == 0 {
            return Err(format!(
                "Windows could not position the strip: {}",
                io::Error::last_os_error()
            ));
        }
    }

    Ok(())
}

pub fn clamp_to_work_area(window: &DoubleWindow, margin: i32) -> Result<(i32, i32), String> {
    let hwnd = window.raw_handle() as HWND;
    if hwnd.is_null() {
        return Err("FLTK did not provide a native strip window handle.".to_owned());
    }

    let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
    if monitor.is_null() {
        return Err("Windows could not identify the nearest monitor.".to_owned());
    }

    let mut monitor_info = MONITORINFO {
        cbSize: mem::size_of::<MONITORINFO>() as u32,
        ..MONITORINFO::default()
    };
    let mut window_rect = RECT::default();

    // SAFETY: The handles are live and both output pointers refer to
    // initialized, correctly sized structures.
    unsafe {
        if GetMonitorInfoW(monitor, &mut monitor_info) == 0
            || GetWindowRect(hwnd, &mut window_rect) == 0
        {
            return Err(format!(
                "Windows could not read the strip placement bounds: {}",
                io::Error::last_os_error()
            ));
        }

        let (x, y) = visible_position(&monitor_info.rcWork, &window_rect, margin);
        if SetWindowPos(hwnd, HWND_TOPMOST, x, y, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE) == 0 {
            return Err(format!(
                "Windows could not keep the strip on-screen: {}",
                io::Error::last_os_error()
            ));
        }

        Ok((x, y))
    }
}

pub fn position_editor_above(
    window: &DoubleWindow,
    anchor: &DoubleWindow,
    gap: i32,
) -> Result<(), String> {
    let hwnd = window.raw_handle() as HWND;
    let anchor_hwnd = anchor.raw_handle() as HWND;
    if hwnd.is_null() || anchor_hwnd.is_null() {
        return Err("FLTK did not provide the native editor and strip window handles.".to_owned());
    }

    // Choose the monitor containing (or nearest to) the strip, since the strip
    // may have been dragged away from the primary display.
    let monitor = unsafe { MonitorFromWindow(anchor_hwnd, MONITOR_DEFAULTTONEAREST) };
    if monitor.is_null() {
        return Err("Windows could not identify the strip's monitor.".to_owned());
    }

    let mut monitor_info = MONITORINFO {
        cbSize: mem::size_of::<MONITORINFO>() as u32,
        ..MONITORINFO::default()
    };
    let mut editor_rect = RECT::default();
    let mut anchor_rect = RECT::default();

    // SAFETY: Both HWNDs belong to live FLTK windows and all output pointers
    // refer to initialized, correctly sized structures.
    unsafe {
        if GetMonitorInfoW(monitor, &mut monitor_info) == 0
            || GetWindowRect(hwnd, &mut editor_rect) == 0
            || GetWindowRect(anchor_hwnd, &mut anchor_rect) == 0
        {
            return Err(format!(
                "Windows could not read the editor placement bounds: {}",
                io::Error::last_os_error()
            ));
        }

        let width = editor_rect.right - editor_rect.left;
        let height = editor_rect.bottom - editor_rect.top;
        let (x, y) = editor_position(&monitor_info.rcWork, &anchor_rect, width, height, gap);

        if SetWindowPos(hwnd, HWND_TOPMOST, x, y, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE) == 0 {
            return Err(format!(
                "Windows could not position the prompt editor: {}",
                io::Error::last_os_error()
            ));
        }
    }

    Ok(())
}

pub fn activate_window(window: &DoubleWindow) -> Result<(), String> {
    let hwnd = window.raw_handle() as HWND;
    if hwnd.is_null() {
        return Err("FLTK did not provide a native editor window handle.".to_owned());
    }

    // SAFETY: `hwnd` belongs to the live FLTK editor window. This call occurs
    // directly in response to a user click, which permits Promplet to request
    // foreground activation under Windows' focus-stealing rules.
    if unsafe { SetForegroundWindow(hwnd) } == 0 {
        return Err("Windows did not bring the prompt editor to the foreground.".to_owned());
    }

    Ok(())
}

pub fn show_strip_menu(
    window: &DoubleWindow,
    x: i32,
    y: i32,
) -> Result<Option<StripMenuAction>, String> {
    const CREATE_ID: usize = 1;
    const SHOW_CONFIG_ID: usize = 2;
    const QUIT_ID: usize = 3;

    let hwnd = window.raw_handle() as HWND;
    if hwnd.is_null() {
        return Err("FLTK did not provide a native strip window handle.".to_owned());
    }

    let create_text = wide_null("New Promplet…");
    let show_config_text = wide_null("Show Config File");
    let quit_text = wide_null("Quit Promplet");

    // SAFETY: The menu is created, used, and destroyed synchronously on the
    // FLTK UI thread. String buffers remain alive until TrackPopupMenuEx
    // returns, and `hwnd` belongs to the live strip window.
    unsafe {
        let menu = CreatePopupMenu();
        if menu.is_null() {
            return Err(format!(
                "Windows could not create the grip menu: {}",
                io::Error::last_os_error()
            ));
        }

        let appended = AppendMenuW(menu, MF_STRING, CREATE_ID, create_text.as_ptr()) != 0
            && AppendMenuW(menu, MF_STRING, SHOW_CONFIG_ID, show_config_text.as_ptr()) != 0
            && AppendMenuW(menu, MF_SEPARATOR, 0, ptr::null()) != 0
            && AppendMenuW(menu, MF_STRING, QUIT_ID, quit_text.as_ptr()) != 0;
        if !appended {
            DestroyMenu(menu);
            return Err(format!(
                "Windows could not populate the grip menu: {}",
                io::Error::last_os_error()
            ));
        }

        let previous_foreground = GetForegroundWindow();
        SetForegroundWindow(hwnd);
        let command = TrackPopupMenuEx(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON,
            x,
            y,
            hwnd,
            ptr::null(),
        );
        PostMessageW(hwnd, WM_NULL, 0, 0);
        DestroyMenu(menu);

        if !previous_foreground.is_null() && previous_foreground != hwnd {
            SetForegroundWindow(previous_foreground);
        }

        match command as usize {
            0 => Ok(None),
            CREATE_ID => Ok(Some(StripMenuAction::Create)),
            SHOW_CONFIG_ID => Ok(Some(StripMenuAction::ShowConfig)),
            QUIT_ID => Ok(Some(StripMenuAction::Quit)),
            unknown => Err(format!(
                "Windows returned an unknown grip menu command: {unknown}"
            )),
        }
    }
}

pub fn reveal_file(path: &Path) -> Result<(), String> {
    if !path.is_file() {
        return Err(format!(
            "The config file does not exist at {}.",
            path.display()
        ));
    }

    Command::new("explorer.exe")
        .arg(explorer_select_argument(path))
        .spawn()
        .map_err(|error| {
            format!(
                "Windows could not reveal {} in Explorer: {error}",
                path.display()
            )
        })?;
    Ok(())
}

pub fn insert_text(text: &str) -> Result<(), String> {
    if text.is_empty() {
        return Ok(());
    }

    // A non-activating strip should never become foreground. Refuse to type if
    // Windows reports no active destination rather than sending input blindly.
    // SAFETY: GetForegroundWindow takes no pointers and has no preconditions.
    if unsafe { GetForegroundWindow() }.is_null() {
        return Err("There is no active destination window.".to_owned());
    }

    let units = plan_text(text);
    let mut inputs = Vec::with_capacity(units.len() * 2);
    for unit in units {
        match unit {
            TextUnit::Return => {
                inputs.push(key_input(VK_RETURN, 0, 0));
                inputs.push(key_input(VK_RETURN, 0, KEYEVENTF_KEYUP));
            }
            TextUnit::Unicode(code_unit) => {
                inputs.push(key_input(0, code_unit, KEYEVENTF_UNICODE));
                inputs.push(key_input(0, code_unit, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP));
            }
        }
    }

    for chunk in inputs.chunks(INPUT_CHUNK_SIZE) {
        // SAFETY: `chunk` points to `INPUT` values with the keyboard union
        // field initialized. Its pointer and byte size remain valid for the
        // duration of the synchronous call.
        let inserted = unsafe {
            SendInput(
                chunk.len() as u32,
                chunk.as_ptr(),
                mem::size_of::<INPUT>() as i32,
            )
        };
        if inserted != chunk.len() as u32 {
            return Err(format!(
                "Windows accepted {inserted} of {} keyboard events. The target may be elevated or may reject synthetic input. ({})",
                chunk.len(),
                io::Error::last_os_error()
            ));
        }
    }

    Ok(())
}

fn foreground_window_is_fullscreen(strip_hwnd: HWND) -> Result<bool, String> {
    let foreground = unsafe { GetForegroundWindow() };
    if foreground.is_null() || foreground == strip_hwnd || is_shell_surface(foreground) {
        return Ok(false);
    }

    let monitor = unsafe { MonitorFromWindow(foreground, MONITOR_DEFAULTTONEAREST) };
    if monitor.is_null() {
        return Ok(false);
    }

    let mut monitor_info = MONITORINFO {
        cbSize: mem::size_of::<MONITORINFO>() as u32,
        ..MONITORINFO::default()
    };
    let mut foreground_rect = RECT::default();
    if unsafe {
        GetMonitorInfoW(monitor, &mut monitor_info) == 0
            || GetWindowRect(foreground, &mut foreground_rect) == 0
    } {
        return Err(format!(
            "Windows could not inspect the foreground window: {}",
            io::Error::last_os_error()
        ));
    }

    Ok(rect_covers(&foreground_rect, &monitor_info.rcMonitor, 2))
}

fn is_shell_surface(hwnd: HWND) -> bool {
    let mut buffer = [0_u16; 64];
    let length = unsafe { GetClassNameW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32) };
    if length <= 0 {
        return false;
    }

    matches!(
        String::from_utf16_lossy(&buffer[..length as usize]).as_str(),
        "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd"
    )
}

fn overlapping_taskbar(strip: &RECT) -> Result<Option<HWND>, String> {
    let primary_class = wide_null("Shell_TrayWnd");
    let primary = unsafe { FindWindowW(primary_class.as_ptr(), ptr::null()) };
    if !primary.is_null() && window_overlaps(primary, strip)? {
        return Ok(Some(primary));
    }

    let secondary_class = wide_null("Shell_SecondaryTrayWnd");
    let mut taskbar = ptr::null_mut();
    loop {
        taskbar = unsafe {
            FindWindowExW(
                ptr::null_mut(),
                taskbar,
                secondary_class.as_ptr(),
                ptr::null(),
            )
        };
        if taskbar.is_null() {
            return Ok(None);
        }
        if window_overlaps(taskbar, strip)? {
            return Ok(Some(taskbar));
        }
    }
}

fn window_overlaps(hwnd: HWND, other: &RECT) -> Result<bool, String> {
    let mut rect = RECT::default();
    if unsafe { GetWindowRect(hwnd, &mut rect) } == 0 {
        return Err(format!(
            "Windows could not inspect the taskbar position: {}",
            io::Error::last_os_error()
        ));
    }
    Ok(rects_overlap(&rect, other))
}

fn window_is_above(candidate: HWND, reference: HWND) -> bool {
    let mut window = unsafe { GetWindow(reference, GW_HWNDPREV) };
    while !window.is_null() {
        if window == candidate {
            return true;
        }
        window = unsafe { GetWindow(window, GW_HWNDPREV) };
    }
    false
}

fn rect_covers(rect: &RECT, bounds: &RECT, tolerance: i32) -> bool {
    let tolerance = tolerance.max(0);
    rect.left <= bounds.left + tolerance
        && rect.top <= bounds.top + tolerance
        && rect.right >= bounds.right - tolerance
        && rect.bottom >= bounds.bottom - tolerance
}

fn rects_overlap(left: &RECT, right: &RECT) -> bool {
    left.left < right.right
        && left.right > right.left
        && left.top < right.bottom
        && left.bottom > right.top
}

fn editor_position(
    work_area: &RECT,
    anchor: &RECT,
    width: i32,
    height: i32,
    gap: i32,
) -> (i32, i32) {
    let max_x = (work_area.right - width).max(work_area.left);
    let max_y = (work_area.bottom - height).max(work_area.top);
    let x = (anchor.right - width).clamp(work_area.left, max_x);

    let gap = gap.max(0);
    let above = anchor.top - gap - height;
    let below = anchor.bottom + gap;
    let preferred_y = if above >= work_area.top {
        above
    } else if below + height <= work_area.bottom {
        below
    } else {
        above
    };

    (x, preferred_y.clamp(work_area.top, max_y))
}

fn visible_position(work_area: &RECT, window: &RECT, margin: i32) -> (i32, i32) {
    let width = window.right - window.left;
    let height = window.bottom - window.top;
    let (min_x, max_x) = axis_bounds(work_area.left, work_area.right, width, margin);
    let (min_y, max_y) = axis_bounds(work_area.top, work_area.bottom, height, margin);

    (
        window.left.clamp(min_x, max_x),
        window.top.clamp(min_y, max_y),
    )
}

fn axis_bounds(start: i32, end: i32, size: i32, margin: i32) -> (i32, i32) {
    let margin = margin.max(0);
    let inset_start = start.saturating_add(margin);
    let inset_end = end.saturating_sub(margin);

    if inset_end.saturating_sub(inset_start) >= size {
        (inset_start, inset_end - size)
    } else {
        (start, (end - size).max(start))
    }
}

fn wide_null(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn explorer_select_argument(path: &Path) -> OsString {
    let mut argument = OsString::from("/select,");
    argument.push(path.as_os_str());
    argument
}

fn key_input(virtual_key: u16, scan_code: u16, flags: u32) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: virtual_key,
                wScan: scan_code,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn plan_text(text: &str) -> Vec<TextUnit> {
    let mut units = Vec::with_capacity(text.len());
    let mut characters = text.chars().peekable();

    while let Some(character) = characters.next() {
        match character {
            '\r' => {
                if characters.peek() == Some(&'\n') {
                    characters.next();
                }
                units.push(TextUnit::Return);
            }
            '\n' => units.push(TextUnit::Return),
            character => {
                let mut encoded = [0_u16; 2];
                units.extend(
                    character
                        .encode_utf16(&mut encoded)
                        .iter()
                        .copied()
                        .map(TextUnit::Unicode),
                );
            }
        }
    }

    units
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_plan_preserves_unicode_surrogate_pairs() {
        assert_eq!(
            plan_text("A😀"),
            vec![
                TextUnit::Unicode('A' as u16),
                TextUnit::Unicode(0xD83D),
                TextUnit::Unicode(0xDE00),
            ]
        );
    }

    #[test]
    fn text_plan_normalizes_all_newline_styles() {
        assert_eq!(
            plan_text("a\r\nb\rc\nd"),
            vec![
                TextUnit::Unicode('a' as u16),
                TextUnit::Return,
                TextUnit::Unicode('b' as u16),
                TextUnit::Return,
                TextUnit::Unicode('c' as u16),
                TextUnit::Return,
                TextUnit::Unicode('d' as u16),
            ]
        );
    }

    #[test]
    fn editor_is_right_aligned_above_anchor() {
        let work_area = RECT {
            left: 0,
            top: 0,
            right: 2560,
            bottom: 1392,
        };
        let anchor = RECT {
            left: 2270,
            top: 1349,
            right: 2548,
            bottom: 1380,
        };

        assert_eq!(
            editor_position(&work_area, &anchor, 520, 390, 8),
            (2028, 951)
        );
    }

    #[test]
    fn editor_stays_inside_monitor_work_area() {
        let work_area = RECT {
            left: -1920,
            top: 0,
            right: 0,
            bottom: 1040,
        };
        let anchor = RECT {
            left: -1915,
            top: 2,
            right: -1700,
            bottom: 33,
        };

        assert_eq!(
            editor_position(&work_area, &anchor, 520, 390, 8),
            (-1920, 41)
        );
    }

    #[test]
    fn hidden_strip_is_recovered_above_the_taskbar() {
        let work_area = RECT {
            left: 0,
            top: 0,
            right: 2560,
            bottom: 1392,
        };
        let hidden_strip = RECT {
            left: 2077,
            top: 1403,
            right: 2355,
            bottom: 1434,
        };

        assert_eq!(
            visible_position(&work_area, &hidden_strip, 12),
            (2077, 1349)
        );
    }

    #[test]
    fn explorer_argument_selects_the_exact_config_file() {
        let path = Path::new(r"C:\Users\Example User\AppData\Local\Promplet\promplets.json");

        assert_eq!(
            explorer_select_argument(path),
            OsString::from(r"/select,C:\Users\Example User\AppData\Local\Promplet\promplets.json")
        );
    }

    #[test]
    fn fullscreen_detection_distinguishes_work_area_maximization() {
        let monitor = RECT {
            left: 0,
            top: 0,
            right: 2560,
            bottom: 1440,
        };
        let fullscreen = RECT {
            left: 0,
            top: 0,
            right: 2560,
            bottom: 1440,
        };
        let maximized = RECT {
            left: -8,
            top: -8,
            right: 2568,
            bottom: 1400,
        };

        assert!(rect_covers(&fullscreen, &monitor, 2));
        assert!(!rect_covers(&maximized, &monitor, 2));
    }

    #[test]
    fn overlap_requires_actual_shared_area() {
        let taskbar = RECT {
            left: 0,
            top: 1392,
            right: 2560,
            bottom: 1440,
        };
        let over_taskbar = RECT {
            left: 2100,
            top: 1402,
            right: 2330,
            bottom: 1433,
        };
        let above_taskbar = RECT {
            left: 2100,
            top: 1361,
            right: 2330,
            bottom: 1392,
        };

        assert!(rects_overlap(&taskbar, &over_taskbar));
        assert!(!rects_overlap(&taskbar, &above_taskbar));
    }
}
