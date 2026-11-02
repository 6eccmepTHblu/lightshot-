//! Opt-in tests that exercise an interactive Windows desktop.
//!
//! Run with:
//! `cargo test --locked --test windows_native -- --ignored --test-threads=1`

#![cfg(target_os = "windows")]
#![allow(unsafe_code)]

use rustshot::{
    frame::{Frame, PhysicalPoint},
    platform::windows::{
        capture::{capture_cursor_monitor, capture_virtual_desktop},
        clipboard::copy_to_clipboard_with_owner,
    },
};
use windows::{
    core::{w, PCWSTR},
    Win32::{
        Foundation::{HGLOBAL, HWND},
        Graphics::Gdi::BITMAPV5HEADER,
        System::{
            DataExchange::{
                CloseClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
                RegisterClipboardFormatW,
            },
            Memory::{GlobalLock, GlobalUnlock},
            Ole::CF_DIBV5,
        },
        UI::WindowsAndMessaging::GetDesktopWindow,
    },
};

#[test]
#[ignore = "requires an unlocked interactive Windows desktop"]
fn cursor_monitor_capture_returns_a_nonempty_valid_frame() {
    let frame = capture_cursor_monitor().expect("cursor monitor capture should succeed");

    assert!(frame.width() > 0);
    assert!(frame.height() > 0);
    assert_eq!(
        frame.rgba().len(),
        frame.width() as usize * frame.height() as usize * 4
    );
}

#[test]
#[ignore = "requires an unlocked interactive Windows desktop"]
fn virtual_desktop_capture_returns_a_nonempty_valid_frame() {
    let frame = capture_virtual_desktop().expect("virtual desktop capture should succeed");

    assert!(frame.width() > 0);
    assert!(frame.height() > 0);
    assert_eq!(
        frame.rgba().len(),
        frame.width() as usize * frame.height() as usize * 4
    );
}

#[test]
#[ignore = "replaces the current Windows clipboard contents"]
fn clipboard_export_publishes_a_dibv5_image() {
    let frame = Frame::new(
        PhysicalPoint::default(),
        2,
        2,
        vec![
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
        ],
    )
    .expect("test frame should be valid");
    // SAFETY: GetDesktopWindow returns a process-independent, always-valid desktop HWND.
    let owner = unsafe { GetDesktopWindow() };

    for allow_history in [false, true] {
        copy_to_clipboard_with_owner(&frame, owner, allow_history)
            .expect("clipboard copy should succeed");
        // SAFETY: this call only queries format availability and owns no resources.
        assert!(unsafe { IsClipboardFormatAvailable(u32::from(CF_DIBV5.0)) }.is_ok());
        assert_eq!(
            read_policy_format(owner, w!("CanUploadToCloudClipboard")),
            0
        );
        assert_eq!(
            read_policy_format(owner, w!("CanIncludeInClipboardHistory")),
            u32::from(allow_history)
        );
        assert_bottom_up_dib(owner);
    }
}

/// The test frame has red and green on top, blue and white below.
fn assert_bottom_up_dib(owner: HWND) {
    // SAFETY: the clipboard stays open while its handle is read, the DIB holds a
    // header followed by 2x2 32-bit pixels, and the lock is balanced before closing.
    unsafe {
        OpenClipboard(Some(owner)).expect("clipboard should open");
        let handle = GetClipboardData(u32::from(CF_DIBV5.0)).expect("DIB should be present");
        let memory = HGLOBAL(handle.0);
        let base = GlobalLock(memory).cast::<u8>();
        let header = base.cast::<BITMAPV5HEADER>().read_unaligned();
        let pixels = std::slice::from_raw_parts(base.add(size_of::<BITMAPV5HEADER>()), 16).to_vec();
        let _ = GlobalUnlock(memory);
        CloseClipboard().expect("clipboard should close");

        assert_eq!((header.bV5Width, header.bV5Height), (2, 2));
        assert_eq!(&pixels[..4], [255, 0, 0, 255], "first stored pixel is blue");
        assert_eq!(
            &pixels[8..12],
            [0, 0, 255, 255],
            "third stored pixel is red"
        );
    }
}

fn read_policy_format(owner: HWND, name: PCWSTR) -> u32 {
    // SAFETY: `name` is a valid null-terminated string, the clipboard stays open
    // while its handle is read, and the lock is balanced before closing.
    unsafe {
        let format = RegisterClipboardFormatW(name);
        OpenClipboard(Some(owner)).expect("clipboard should open");
        let handle = GetClipboardData(format).expect("policy format should be present");
        let memory = HGLOBAL(handle.0);
        let value = *GlobalLock(memory).cast::<u32>();
        let _ = GlobalUnlock(memory);
        CloseClipboard().expect("clipboard should close");
        value
    }
}
