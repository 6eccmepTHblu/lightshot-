//! Lets the capture overlay appear at once instead of through the system animation.

#![allow(unsafe_code)]

use std::{ffi::c_void, mem::size_of, ptr::from_ref};

use windows::Win32::{
    Foundation::HWND,
    Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_CLOAK, DWMWA_TRANSITIONS_FORCEDISABLED, DWMWINDOWATTRIBUTE,
    },
};

fn set_flag(hwnd: HWND, attribute: DWMWINDOWATTRIBUTE, enabled: bool) -> bool {
    let value = i32::from(enabled);
    // SAFETY: `hwnd` is a live window owned by this process, and both attributes
    // take a BOOL, which `value` provides for the duration of the call.
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            attribute,
            from_ref(&value).cast::<c_void>(),
            size_of::<i32>() as u32,
        )
    }
    .is_ok()
}

/// Turns off the open and close animation and hides the window until its first
/// frame exists. Returns whether [`reveal`] is needed to make the window visible.
pub fn prepare_instant_show(hwnd: HWND) -> bool {
    set_flag(hwnd, DWMWA_TRANSITIONS_FORCEDISABLED, true);
    set_flag(hwnd, DWMWA_CLOAK, true)
}

/// Shows a window hidden by [`prepare_instant_show`].
pub fn reveal(hwnd: HWND) {
    set_flag(hwnd, DWMWA_CLOAK, false);
}
