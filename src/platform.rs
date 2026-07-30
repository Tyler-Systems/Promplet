#[cfg(windows)]
mod windows;

#[cfg(windows)]
pub use windows::{
    StripMenuAction, activate_window, claim_single_instance, clamp_to_work_area,
    configure_strip_window, insert_text, maintain_strip_z_order, position_bottom_right,
    position_editor, reveal_file, show_strip_menu,
};

#[cfg(not(windows))]
compile_error!("The current Promplet prototype only supports Windows.");
