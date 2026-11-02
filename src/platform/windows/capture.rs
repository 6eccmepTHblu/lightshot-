//! One-shot Windows monitor capture with optional native cursor compositing.

#![allow(unsafe_code)]

use std::{mem::size_of, ptr::NonNull, slice};

use anyhow::{bail, Context, Result};
use windows::{
    core::Error as WindowsError,
    Win32::{
        Foundation::POINT,
        Graphics::Gdi::{
            CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GdiFlush, SelectObject,
            BITMAPINFO, BI_RGB, DIB_RGB_COLORS, HBITMAP, HDC, HGDIOBJ,
        },
        UI::WindowsAndMessaging::{
            DrawIconEx, GetCursorInfo, GetCursorPos, GetIconInfo, CURSORINFO, CURSOR_SHOWING,
            DI_NORMAL, HICON, ICONINFO,
        },
    },
};
use xcap::Monitor;

use crate::frame::{Frame, PhysicalPoint};

const RGBA_CHANNELS: usize = 4;

/// Captures the complete monitor containing the mouse cursor.
pub fn capture_cursor_monitor() -> Result<Frame> {
    capture_cursor_monitor_with_cursor(false)
}

/// Captures the monitor under the cursor and optionally includes the pointer.
pub fn capture_cursor_monitor_with_cursor(include_cursor: bool) -> Result<Frame> {
    let cursor = current_cursor_position()?;
    capture_monitor_at_with_cursor(cursor.x(), cursor.y(), include_cursor)
}

/// Compatibility name for callers that describe the same operation in UI terms.
pub fn capture_monitor_under_cursor() -> Result<Frame> {
    capture_cursor_monitor()
}

/// Reads the current physical desktop cursor position.
pub fn current_cursor_position() -> Result<PhysicalPoint> {
    let cursor = cursor_position()?;
    Ok(PhysicalPoint::new(cursor.x, cursor.y))
}

/// Captures the complete monitor containing the supplied physical desktop point.
pub fn capture_monitor_at(x: i32, y: i32) -> Result<Frame> {
    capture_monitor_at_with_cursor(x, y, false)
}

/// Captures the monitor containing a previously sampled desktop point.
pub fn capture_monitor_at_with_cursor(x: i32, y: i32, include_cursor: bool) -> Result<Frame> {
    let monitor = Monitor::from_point(x, y)
        .with_context(|| format!("в точке ({x}, {y}) нет монитора, доступного для снимка"))?;
    let frame = capture_monitor(&monitor)?;
    include_cursor_if_requested(frame, include_cursor)
}

/// Captures every attached monitor and composites them into one virtual-desktop
/// frame. Gaps between monitors are opaque black.
pub fn capture_virtual_desktop() -> Result<Frame> {
    capture_virtual_desktop_with_cursor(false)
}

/// Captures the virtual desktop and optionally composites the native pointer.
pub fn capture_virtual_desktop_with_cursor(include_cursor: bool) -> Result<Frame> {
    let monitors = Monitor::all().context("не удалось получить список мониторов")?;
    if monitors.is_empty() {
        bail!("Windows не сообщила ни об одном мониторе, доступном для снимка");
    }

    let mut captures = Vec::with_capacity(monitors.len());
    let mut left = i64::MAX;
    let mut top = i64::MAX;
    let mut right = i64::MIN;
    let mut bottom = i64::MIN;

    for monitor in monitors {
        let x = monitor
            .x()
            .context("не удалось определить координату X монитора")?;
        let y = monitor
            .y()
            .context("не удалось определить координату Y монитора")?;
        let image = monitor
            .capture_image()
            .with_context(|| format!("не удалось снять монитор с началом в ({x}, {y})"))?;
        let width = image.width();
        let height = image.height();
        if width == 0 || height == 0 {
            bail!("монитор в ({x}, {y}) вернул пустой снимок");
        }

        let monitor_right = i64::from(x) + i64::from(width);
        let monitor_bottom = i64::from(y) + i64::from(height);
        left = left.min(i64::from(x));
        top = top.min(i64::from(y));
        right = right.max(monitor_right);
        bottom = bottom.max(monitor_bottom);
        captures.push(CapturedMonitor {
            x,
            y,
            width,
            height,
            rgba: image.into_raw(),
        });
    }

    let width = u32::try_from(right - left)
        .context("ширина виртуального рабочего стола вне допустимого диапазона")?;
    let height = u32::try_from(bottom - top)
        .context("высота виртуального рабочего стола вне допустимого диапазона")?;
    let origin = PhysicalPoint::new(
        i32::try_from(left)
            .context("начало виртуального рабочего стола по X вне допустимого диапазона")?,
        i32::try_from(top)
            .context("начало виртуального рабочего стола по Y вне допустимого диапазона")?,
    );
    let byte_len = rgba_len(width, height)?;
    let mut rgba = vec![0_u8; byte_len];

    // Pixels outside real monitor rectangles should remain visible black rather
    // than transparent when the virtual desktop has an irregular shape.
    for alpha in rgba.iter_mut().skip(3).step_by(RGBA_CHANNELS) {
        *alpha = u8::MAX;
    }

    let destination_stride = usize::try_from(width)
        .context("ширина виртуального рабочего стола слишком велика для этой платформы")?
        .checked_mul(RGBA_CHANNELS)
        .context("переполнение размера строки виртуального рабочего стола")?;

    for capture in captures {
        composite_monitor(&capture, left, top, destination_stride, &mut rgba)?;
    }

    let frame = Frame::new(origin, width, height, rgba)
        .context("неверный кадр виртуального рабочего стола")?;
    include_cursor_if_requested(frame, include_cursor)
}

fn cursor_position() -> Result<POINT> {
    let mut point = POINT::default();
    // SAFETY: `point` is a valid, writable POINT for the duration of the call.
    unsafe { GetCursorPos(&mut point) }.context("не удалось определить положение курсора")?;
    Ok(point)
}

fn include_cursor_if_requested(frame: Frame, include_cursor: bool) -> Result<Frame> {
    if include_cursor {
        composite_cursor(frame)
    } else {
        Ok(frame)
    }
}

fn composite_cursor(frame: Frame) -> Result<Frame> {
    let mut cursor = CURSORINFO {
        cbSize: u32::try_from(size_of::<CURSORINFO>())
            .expect("CURSORINFO structure size always fits u32"),
        ..Default::default()
    };
    // SAFETY: `cursor` has the documented size and remains writable for the call.
    unsafe { GetCursorInfo(&mut cursor) }.context("не удалось получить изображение курсора")?;
    if cursor.flags.0 & CURSOR_SHOWING.0 == 0 || cursor.hCursor.is_invalid() {
        return Ok(frame);
    }

    let mut icon = ICONINFO::default();
    // SAFETY: the cursor handle came from GetCursorInfo and `icon` is writable.
    unsafe { GetIconInfo(HICON(cursor.hCursor.0), &mut icon) }
        .context("не удалось разобрать изображение курсора")?;
    let icon = OwnedIconInfo(icon);

    let hotspot_x = i64::from(icon.0.xHotspot);
    let hotspot_y = i64::from(icon.0.yHotspot);
    let draw_x = i64::from(cursor.ptScreenPos.x) - i64::from(frame.origin().x()) - hotspot_x;
    let draw_y = i64::from(cursor.ptScreenPos.y) - i64::from(frame.origin().y()) - hotspot_y;
    let (Ok(draw_x), Ok(draw_y)) = (i32::try_from(draw_x), i32::try_from(draw_y)) else {
        return Ok(frame);
    };

    let origin = frame.origin();
    let width = frame.width();
    let height = frame.height();
    let mut rgba = frame.into_rgba();
    let mut dib = DibSurface::new(width, height, rgba.len())?;
    for (source, destination) in rgba
        .chunks_exact(4)
        .zip(dib.pixels_mut().chunks_exact_mut(4))
    {
        destination.copy_from_slice(&[source[2], source[1], source[0], u8::MAX]);
    }

    // SAFETY: the memory DC has a selected 32-bit DIB, the cursor handle is
    // valid for this call, and GDI clips drawing outside the DIB bounds.
    unsafe {
        DrawIconEx(
            dib.dc,
            draw_x,
            draw_y,
            HICON(cursor.hCursor.0),
            0,
            0,
            0,
            None,
            DI_NORMAL,
        )
    }
    .context("не удалось наложить курсор на снимок")?;
    // SAFETY: flushes this thread's queued GDI drawing before direct DIB access.
    if !unsafe { GdiFlush() }.as_bool() {
        return Err(WindowsError::from_thread()).context("не удалось завершить отрисовку курсора");
    }

    for (source, destination) in dib
        .pixels_mut()
        .chunks_exact(4)
        .zip(rgba.chunks_exact_mut(4))
    {
        destination.copy_from_slice(&[source[2], source[1], source[0], u8::MAX]);
    }
    Frame::new(origin, width, height, rgba).context("наложение курсора дало неверный кадр")
}

fn capture_monitor(monitor: &Monitor) -> Result<Frame> {
    let x = monitor
        .x()
        .context("не удалось определить координату X монитора")?;
    let y = monitor
        .y()
        .context("не удалось определить координату Y монитора")?;
    let image = monitor
        .capture_image()
        .with_context(|| format!("не удалось снять монитор в ({x}, {y})"))?;

    Frame::new(
        PhysicalPoint::new(x, y),
        image.width(),
        image.height(),
        image.into_raw(),
    )
    .context("снимок монитора вернул неверный кадр")
}

fn composite_monitor(
    capture: &CapturedMonitor,
    virtual_left: i64,
    virtual_top: i64,
    destination_stride: usize,
    destination: &mut [u8],
) -> Result<()> {
    let source_stride = usize::try_from(capture.width)
        .context("ширина монитора слишком велика для этой платформы")?
        .checked_mul(RGBA_CHANNELS)
        .context("переполнение размера строки монитора")?;
    let expected_source_len = rgba_len(capture.width, capture.height)?;
    if capture.rgba.len() != expected_source_len {
        bail!(
            "в буфере снимка монитора {} байт, ожидалось {expected_source_len}",
            capture.rgba.len()
        );
    }

    let x_offset = usize::try_from(i64::from(capture.x) - virtual_left)
        .context("у монитора неверное смещение по X на виртуальном рабочем столе")?;
    let y_offset = usize::try_from(i64::from(capture.y) - virtual_top)
        .context("у монитора неверное смещение по Y на виртуальном рабочем столе")?;
    let x_bytes = x_offset
        .checked_mul(RGBA_CHANNELS)
        .context("переполнение смещения монитора по X")?;
    let row_count = usize::try_from(capture.height)
        .context("высота монитора слишком велика для этой платформы")?;

    for row in 0..row_count {
        let source_start = row
            .checked_mul(source_stride)
            .context("переполнение смещения исходной строки монитора")?;
        let source_end = source_start
            .checked_add(source_stride)
            .context("переполнение конца исходной строки монитора")?;
        let destination_start = y_offset
            .checked_add(row)
            .and_then(|row| row.checked_mul(destination_stride))
            .and_then(|row| row.checked_add(x_bytes))
            .context("переполнение смещения целевой строки монитора")?;
        let destination_end = destination_start
            .checked_add(source_stride)
            .context("переполнение конца целевой строки монитора")?;
        let output = destination
            .get_mut(destination_start..destination_end)
            .context("монитор находится за пределами буфера виртуального рабочего стола")?;
        output.copy_from_slice(&capture.rgba[source_start..source_end]);
    }

    Ok(())
}

fn rgba_len(width: u32, height: u32) -> Result<usize> {
    usize::try_from(width)
        .ok()
        .and_then(|width| {
            usize::try_from(height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .and_then(|pixels| pixels.checked_mul(RGBA_CHANNELS))
        .context("переполнение размера буфера снимка")
}

struct CapturedMonitor {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

struct OwnedIconInfo(ICONINFO);

impl Drop for OwnedIconInfo {
    fn drop(&mut self) {
        for bitmap in [self.0.hbmMask, self.0.hbmColor] {
            if !bitmap.is_invalid() {
                // SAFETY: GetIconInfo allocated these bitmaps for the caller.
                let _ = unsafe { DeleteObject(HGDIOBJ(bitmap.0)) };
            }
        }
    }
}

struct DibSurface {
    dc: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
    bits: NonNull<u8>,
    len: usize,
}

impl DibSurface {
    fn new(width: u32, height: u32, len: usize) -> Result<Self> {
        let width = i32::try_from(width).context("ширина снимка превышает предел GDI")?;
        let height = i32::try_from(height).context("высота снимка превышает предел GDI")?;
        let image_size = u32::try_from(len).context("снимок слишком велик для изображения GDI")?;
        let mut info = BITMAPINFO::default();
        info.bmiHeader.biSize = u32::try_from(size_of_val(&info.bmiHeader))
            .expect("BITMAPINFOHEADER structure size always fits u32");
        info.bmiHeader.biWidth = width;
        info.bmiHeader.biHeight = -height;
        info.bmiHeader.biPlanes = 1;
        info.bmiHeader.biBitCount = 32;
        info.bmiHeader.biCompression = BI_RGB.0;
        info.bmiHeader.biSizeImage = image_size;

        // SAFETY: a null source DC requests a memory DC compatible with the screen.
        let dc = unsafe { CreateCompatibleDC(None) };
        if dc.is_invalid() {
            return Err(WindowsError::from_thread())
                .context("не удалось создать контекст памяти для курсора");
        }

        let mut bits = std::ptr::null_mut();
        // SAFETY: `info` describes a valid top-down 32-bit DIB and `bits` is a
        // writable output pointer. No shared file mapping is used.
        let bitmap = match unsafe {
            CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0)
        } {
            Ok(bitmap) => bitmap,
            Err(error) => {
                // SAFETY: `dc` was created above and is not selected elsewhere.
                let _ = unsafe { DeleteDC(dc) };
                return Err(error).context("не удалось создать изображение для наложения курсора");
            }
        };
        let Some(bits) = NonNull::new(bits.cast::<u8>()) else {
            // SAFETY: these resources were created above and are still unselected.
            let _ = unsafe { DeleteObject(HGDIOBJ(bitmap.0)) };
            let _ = unsafe { DeleteDC(dc) };
            bail!("изображение для наложения курсора не получило память для пикселей");
        };

        // SAFETY: both handles are valid and the bitmap is not selected into
        // another DC. The returned previous object is restored by Drop.
        let previous = unsafe { SelectObject(dc, HGDIOBJ(bitmap.0)) };
        if previous.is_invalid() {
            // SAFETY: selection failed, so the bitmap and DC remain independently owned.
            let _ = unsafe { DeleteObject(HGDIOBJ(bitmap.0)) };
            let _ = unsafe { DeleteDC(dc) };
            return Err(WindowsError::from_thread())
                .context("не удалось выбрать изображение для наложения курсора");
        }

        Ok(Self {
            dc,
            bitmap,
            previous,
            bits,
            len,
        })
    }

    fn pixels_mut(&mut self) -> &mut [u8] {
        // SAFETY: CreateDIBSection allocated exactly `len` bytes for this 32-bit
        // bitmap, and `self` uniquely owns access until it is dropped.
        unsafe { slice::from_raw_parts_mut(self.bits.as_ptr(), self.len) }
    }
}

impl Drop for DibSurface {
    fn drop(&mut self) {
        // SAFETY: all handles are owned by this guard; restoring the previous
        // selection is required before deleting the bitmap and its memory DC.
        unsafe {
            let _ = SelectObject(self.dc, self.previous);
            let _ = DeleteObject(HGDIOBJ(self.bitmap.0));
            let _ = DeleteDC(self.dc);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{composite_monitor, rgba_len, CapturedMonitor};

    #[test]
    fn monitor_pixels_are_composited_at_their_virtual_desktop_offset() {
        let capture = CapturedMonitor {
            x: -1,
            y: 2,
            width: 2,
            height: 1,
            rgba: vec![1, 2, 3, 4, 5, 6, 7, 8],
        };
        let mut destination = vec![0; 4 * 2 * 4];

        composite_monitor(&capture, -2, 1, 4 * 4, &mut destination)
            .expect("valid monitor should composite");

        assert_eq!(&destination[20..28], capture.rgba.as_slice());
        assert!(destination[..20].iter().all(|byte| *byte == 0));
        assert!(destination[28..].iter().all(|byte| *byte == 0));
    }

    #[test]
    fn monitor_compositing_rejects_a_malformed_capture_buffer() {
        let capture = CapturedMonitor {
            x: 0,
            y: 0,
            width: 2,
            height: 2,
            rgba: vec![0; 3],
        };
        let mut destination = vec![0; 2 * 2 * 4];

        assert!(composite_monitor(&capture, 0, 0, 2 * 4, &mut destination).is_err());
    }

    #[test]
    fn rgba_length_checks_multiplication_overflow() {
        assert_eq!(rgba_len(2, 3).expect("small image should fit"), 24);
        assert!(rgba_len(u32::MAX, u32::MAX).is_err());
    }
}
