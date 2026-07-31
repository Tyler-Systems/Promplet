mod geometry;
mod text;

#[cfg(windows)]
mod windows;

#[cfg(windows)]
pub use windows::{
    activate_window, claim_single_instance, clamp_to_work_area, configure_strip_window,
    insert_text, maintain_strip_z_order, position_bottom_right, position_editor,
    release_activation, reveal_file, show_strip_menu,
};

#[cfg(target_os = "macos")]
mod mac;

#[cfg(target_os = "macos")]
pub use mac::{
    activate_window, claim_single_instance, clamp_to_work_area, configure_strip_window,
    insert_text, maintain_strip_z_order, position_bottom_right, position_editor,
    release_activation, reveal_file, show_strip_menu,
};

#[cfg(target_os = "linux")]
mod linux;

#[cfg(target_os = "linux")]
pub use linux::{
    activate_window, claim_single_instance, clamp_to_work_area, configure_strip_window,
    insert_text, maintain_strip_z_order, position_bottom_right, position_editor,
    release_activation, reveal_file, show_strip_menu,
};

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
compile_error!("The current Promplet prototype supports Windows, macOS, and Linux/X11.");

/// An action chosen from the grip context menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StripMenuAction {
    Create,
    ToggleOrientation,
    ShowConfig,
    ReloadConfig,
    Quit,
}
