//! Windows keyboard settings that affect RustShot's global shortcuts.

#![allow(unsafe_code)]

use std::{ffi::c_void, mem::size_of};

use windows::{
    core::w,
    Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD},
};

/// Reports whether Windows reserves the Print Screen key for its screen capture.
/// Windows 11 enables this until the user changes it, so a missing value counts.
#[must_use]
pub fn print_screen_opens_snipping_tool() -> bool {
    let mut value = 1_u32;
    let mut size = size_of::<u32>() as u32;
    // SAFETY: the name strings are null-terminated, and `value` and `size`
    // outlive the call and describe a writable DWORD.
    let _ = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!(r"Control Panel\Keyboard"),
            w!("PrintScreenKeyForSnippingEnabled"),
            RRF_RT_REG_DWORD,
            None,
            Some(std::ptr::from_mut(&mut value).cast::<c_void>()),
            Some(&mut size),
        )
    };
    value != 0
}
