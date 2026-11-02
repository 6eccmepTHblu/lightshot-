//! Windows clipboard export using the color-managed `CF_DIBV5` format.

#![allow(unsafe_code)]

use std::{mem::size_of, ptr, slice, thread, time::Duration};

use anyhow::{bail, Context, Result};
use windows::{
    core::{w, Error as WindowsError, PCWSTR},
    Win32::{
        Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND},
        Graphics::Gdi::{BITMAPV5HEADER, BI_BITFIELDS},
        System::{
            DataExchange::{
                CloseClipboard, EmptyClipboard, OpenClipboard, RegisterClipboardFormatW,
                SetClipboardData,
            },
            Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE},
            Ole::CF_DIBV5,
        },
    },
};

use crate::frame::Frame;

const RGBA_CHANNELS: usize = 4;
const OPEN_ATTEMPTS: usize = 10;
const OPEN_RETRY_DELAY: Duration = Duration::from_millis(10);
const LCS_SRGB: u32 = 0x7352_4742;
const LCS_GM_IMAGES: u32 = 4;

/// Copies a frame to the Windows clipboard using a real Rustshot owner window.
///
/// Cloud clipboard sync is always refused. `allow_history` controls whether the
/// screenshot may appear in the local Win+V history.
pub fn copy_to_clipboard_with_owner(frame: &Frame, owner: HWND, allow_history: bool) -> Result<()> {
    if owner.is_invalid() {
        bail!("окно-владелец буфера обмена недействительно");
    }
    let mut dib = create_dibv5(frame)?;
    let _clipboard = ClipboardGuard::open(owner)?;

    // SAFETY: the guard proves that this thread currently has the clipboard open.
    unsafe { EmptyClipboard() }.context("не удалось очистить буфер обмена Windows")?;

    // These go first so a failure leaves no image on the clipboard unprotected.
    set_policy_format(w!("CanUploadToCloudClipboard"), false)?;
    set_policy_format(w!("CanIncludeInClipboardHistory"), allow_history)?;

    let handle = dib
        .handle
        .context("память изображения для буфера обмена неожиданно освобождена")?;
    // SAFETY: `handle` is movable global memory containing a valid CF_DIBV5
    // payload. On success Windows assumes ownership, and `release` prevents a
    // second free from Rust.
    unsafe { SetClipboardData(u32::from(CF_DIBV5.0), Some(HANDLE(handle.0))) }
        .context("не удалось поместить снимок в буфер обмена Windows")?;
    dib.release();
    Ok(())
}

/// Places a documented clipboard policy format holding a single DWORD. The
/// clipboard must be open on this thread.
fn set_policy_format(name: PCWSTR, allowed: bool) -> Result<()> {
    // SAFETY: `name` is a valid null-terminated string from the `w!` macro.
    let format = unsafe { RegisterClipboardFormatW(name) };
    if format == 0 {
        return Err(WindowsError::from_thread())
            .context("не удалось зарегистрировать формат буфера обмена");
    }

    // SAFETY: the allocation is managed by `OwnedGlobalMemory` until Windows owns it.
    let memory = unsafe { GlobalAlloc(GMEM_MOVEABLE, size_of::<u32>()) }
        .context("не удалось выделить память для буфера обмена")?;
    let mut memory = OwnedGlobalMemory::new(memory);

    // SAFETY: the handle is valid movable global memory allocated above.
    let base = unsafe { GlobalLock(memory.handle()) }.cast::<u32>();
    if base.is_null() {
        return Err(WindowsError::from_thread())
            .context("не удалось заблокировать память буфера обмена");
    }
    // SAFETY: `base` addresses at least one writable, suitably aligned u32, and
    // the unlock balances the successful `GlobalLock` above. See `create_dibv5`
    // for why the unlock result is ignored.
    unsafe {
        ptr::write(base, u32::from(allowed));
        let _ = GlobalUnlock(memory.handle());
    }

    // SAFETY: the payload is valid and the clipboard is open. On success Windows
    // assumes ownership, and `release` prevents a second free from Rust.
    unsafe { SetClipboardData(format, Some(HANDLE(memory.handle().0))) }
        .context("не удалось задать запрет для буфера обмена")?;
    memory.release();
    Ok(())
}

fn create_dibv5(frame: &Frame) -> Result<OwnedGlobalMemory> {
    let width =
        i32::try_from(frame.width()).context("ширина изображения превышает предел формата DIB")?;
    let height =
        i32::try_from(frame.height()).context("высота изображения превышает предел формата DIB")?;
    let image_bytes = frame.rgba().len();
    let image_size =
        u32::try_from(image_bytes).context("изображение слишком велико для формата DIB")?;
    let header_size = size_of::<BITMAPV5HEADER>();
    let allocation_size = header_size
        .checked_add(image_bytes)
        .context("переполнение размера памяти для буфера обмена")?;

    // SAFETY: allocation size is checked and the returned handle is managed by
    // `OwnedGlobalMemory` until Windows takes ownership.
    let memory = unsafe { GlobalAlloc(GMEM_MOVEABLE, allocation_size) }
        .context("не удалось выделить память для буфера обмена")?;
    let memory = OwnedGlobalMemory::new(memory);

    // SAFETY: the handle is valid movable global memory allocated above.
    let base = unsafe { GlobalLock(memory.handle()) }.cast::<u8>();
    if base.is_null() {
        return Err(WindowsError::from_thread())
            .context("не удалось заблокировать память буфера обмена");
    }

    let header = BITMAPV5HEADER {
        bV5Size: u32::try_from(header_size).expect("BITMAPV5HEADER size always fits u32"),
        bV5Width: width,
        // Bottom-up, the conventional row order. Some clipboard managers cannot
        // show the negative-height top-down form, though it is valid.
        bV5Height: height,
        bV5Planes: 1,
        bV5BitCount: 32,
        bV5Compression: BI_BITFIELDS,
        bV5SizeImage: image_size,
        bV5RedMask: 0x00ff_0000,
        bV5GreenMask: 0x0000_ff00,
        bV5BlueMask: 0x0000_00ff,
        bV5AlphaMask: 0xff00_0000,
        bV5CSType: LCS_SRGB,
        bV5Intent: LCS_GM_IMAGES,
        ..Default::default()
    };

    // SAFETY: `base` addresses `allocation_size` writable bytes. The header and
    // following pixel slice are non-overlapping and exactly fill that allocation.
    unsafe {
        ptr::write(base.cast::<BITMAPV5HEADER>(), header);
        let output = slice::from_raw_parts_mut(base.add(header_size), image_bytes);
        let row_bytes = usize::try_from(frame.width())
            .context("ширина изображения превышает предел формата DIB")?
            * RGBA_CHANNELS;
        top_down_rgba_to_bottom_up_bgra(frame.rgba(), output, row_bytes)?;
    }

    // GlobalUnlock reports zero both for a successful final unlock and for an
    // error, so the windows-rs Result is intentionally ignored here.
    // SAFETY: this balances the successful `GlobalLock` above.
    let _ = unsafe { GlobalUnlock(memory.handle()) };
    Ok(memory)
}

fn top_down_rgba_to_bottom_up_bgra(
    source: &[u8],
    destination: &mut [u8],
    row_bytes: usize,
) -> Result<()> {
    if source.len() != destination.len()
        || row_bytes == 0
        || !row_bytes.is_multiple_of(RGBA_CHANNELS)
        || !source.len().is_multiple_of(row_bytes)
    {
        bail!("неверная длина буфера RGBA для буфера обмена");
    }

    for (source_row, destination_row) in source
        .chunks_exact(row_bytes)
        .zip(destination.chunks_exact_mut(row_bytes).rev())
    {
        for (rgba, bgra) in source_row
            .chunks_exact(RGBA_CHANNELS)
            .zip(destination_row.chunks_exact_mut(RGBA_CHANNELS))
        {
            bgra.copy_from_slice(&[rgba[2], rgba[1], rgba[0], rgba[3]]);
        }
    }
    Ok(())
}

struct ClipboardGuard;

impl ClipboardGuard {
    fn open(owner: HWND) -> Result<Self> {
        let mut last_error = None;
        for attempt in 0..OPEN_ATTEMPTS {
            // SAFETY: `owner` is the live Rustshot overlay HWND.
            match unsafe { OpenClipboard(Some(owner)) } {
                Ok(()) => return Ok(Self),
                Err(error) => last_error = Some(error),
            }
            if attempt + 1 < OPEN_ATTEMPTS {
                thread::sleep(OPEN_RETRY_DELAY);
            }
        }

        let error = last_error.unwrap_or_else(WindowsError::from_thread);
        Err(error).context("буфер обмена Windows занят")
    }
}

impl Drop for ClipboardGuard {
    fn drop(&mut self) {
        // SAFETY: the guard is created only after OpenClipboard succeeds and is
        // neither cloneable nor movable across threads.
        let _ = unsafe { CloseClipboard() };
    }
}

struct OwnedGlobalMemory {
    handle: Option<HGLOBAL>,
}

impl OwnedGlobalMemory {
    const fn new(handle: HGLOBAL) -> Self {
        Self {
            handle: Some(handle),
        }
    }

    fn handle(&self) -> HGLOBAL {
        self.handle
            .expect("global memory handle must exist while owned")
    }

    fn release(&mut self) {
        self.handle = None;
    }
}

impl Drop for OwnedGlobalMemory {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            // SAFETY: this object has unique ownership unless `release` was called.
            let _ = unsafe { GlobalFree(Some(handle)) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::PhysicalPoint;

    #[test]
    fn converts_rgba_to_bgra_within_a_row() {
        let mut output = [0_u8; 8];
        top_down_rgba_to_bottom_up_bgra(&[10, 20, 30, 40, 50, 60, 70, 80], &mut output, 8).unwrap();
        assert_eq!(output, [30, 20, 10, 40, 70, 60, 50, 80]);
    }

    #[test]
    fn reverses_the_row_order() {
        let mut output = [0_u8; 8];
        top_down_rgba_to_bottom_up_bgra(&[1, 2, 3, 4, 5, 6, 7, 8], &mut output, 4).unwrap();
        assert_eq!(output, [7, 6, 5, 8, 3, 2, 1, 4]);
    }

    #[test]
    fn rejects_a_buffer_that_is_not_whole_rows() {
        let mut output = [0_u8; 8];
        assert!(top_down_rgba_to_bottom_up_bgra(&[0; 8], &mut output, 12).is_err());
        assert!(top_down_rgba_to_bottom_up_bgra(&[0; 8], &mut output, 0).is_err());
    }

    #[test]
    fn rejects_a_null_owner_without_touching_the_clipboard() {
        let frame = Frame::new(PhysicalPoint::default(), 1, 1, vec![0, 0, 0, 255]).unwrap();
        let error = copy_to_clipboard_with_owner(&frame, HWND::default(), false).unwrap_err();
        assert!(error.to_string().contains("окно-владелец буфера обмена"));
    }
}
