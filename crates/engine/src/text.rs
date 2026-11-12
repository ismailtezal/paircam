//! Draws a short line of text into a coverage mask, for the message on the away screen.
//!
//! Windows draws it (GDI), so every script Windows has a font for works, with the system's
//! font fallback. The text is drawn at twice the size and averaged down, which gives smoother
//! edges than GDI's own antialiasing at these sizes.

/// How much of each pixel the text covers: 0 for none, 255 for all. Tightly packed rows.
pub(crate) struct TextMask {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) coverage: Vec<u8>,
}

/// Draws `text` centred, `pixel_height` tall, wrapped to at most `max_width` pixels. Returns
/// `None` for blank text or when it cannot be drawn.
pub(crate) fn draw(text: &str, pixel_height: u32, max_width: u32) -> Option<TextMask> {
    if text.trim().is_empty() || pixel_height == 0 || max_width == 0 {
        return None;
    }
    platform::draw(text, pixel_height, max_width)
}

#[cfg(windows)]
mod platform {
    use std::ffi::c_void;

    use windows::Win32::Foundation::{COLORREF, RECT};
    use windows::Win32::Graphics::Gdi::{
        ANTIALIASED_QUALITY, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CLIP_DEFAULT_PRECIS,
        CreateCompatibleDC, CreateDIBSection, CreateFontW, DEFAULT_CHARSET, DIB_RGB_COLORS,
        DT_CALCRECT, DT_CENTER, DT_NOPREFIX, DT_WORDBREAK, DeleteDC, DeleteObject, DrawTextW,
        GdiFlush, HDC, HGDIOBJ, OUT_TT_PRECIS, SelectObject, SetBkMode, SetTextColor, TRANSPARENT,
    };
    use windows::core::w;

    use super::TextMask;

    /// The text is drawn this many times larger and averaged down.
    const OVERSAMPLE: u32 = 2;
    /// Semibold, as headings are in the app.
    const WEIGHT: i32 = 600;
    /// The largest mask, so an odd request cannot ask for a huge bitmap.
    const MAX_SIDE: i32 = 8192;

    /// Deletes the device context and whatever was made for it, in the right order.
    struct Canvas {
        dc: HDC,
        made: Vec<HGDIOBJ>,
        original: Vec<HGDIOBJ>,
    }

    impl Canvas {
        /// Selects `object` and remembers to put the previous one back and delete this one.
        ///
        /// # Safety
        /// `object` must be a live GDI object that nothing else deletes.
        unsafe fn select(&mut self, object: HGDIOBJ) {
            // SAFETY: the device context is live and the caller vouches for the object.
            let previous = unsafe { SelectObject(self.dc, object) };
            self.original.push(previous);
            self.made.push(object);
        }
    }

    impl Drop for Canvas {
        fn drop(&mut self) {
            // SAFETY: every handle here was created by this module and is deleted once, after
            // the original objects are selected back so nothing in use is deleted.
            unsafe {
                for original in self.original.drain(..).rev() {
                    SelectObject(self.dc, original);
                }
                for made in self.made.drain(..) {
                    let _ = DeleteObject(made);
                }
                let _ = DeleteDC(self.dc);
            }
        }
    }

    pub(super) fn draw(text: &str, pixel_height: u32, max_width: u32) -> Option<TextMask> {
        let mut wide: Vec<u16> = text.encode_utf16().collect();
        let format = DT_CENTER | DT_WORDBREAK | DT_NOPREFIX;

        // SAFETY: every GDI object is created, used and deleted on this thread within this
        // call; the bitmap's memory is read only while the bitmap is alive, and its size is
        // the one the bitmap was created with.
        unsafe {
            let dc = CreateCompatibleDC(None);
            if dc.is_invalid() {
                return None;
            }
            let mut canvas = Canvas {
                dc,
                made: Vec::new(),
                original: Vec::new(),
            };

            let font = CreateFontW(
                -((pixel_height * OVERSAMPLE) as i32),
                0,
                0,
                0,
                WEIGHT,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_TT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                ANTIALIASED_QUALITY,
                0,
                // Windows substitutes its default interface font where this one is missing.
                w!("Segoe UI Variable Display"),
            );
            if font.is_invalid() {
                return None;
            }
            canvas.select(font.into());

            let mut bounds = RECT {
                left: 0,
                top: 0,
                right: (max_width * OVERSAMPLE) as i32,
                bottom: 0,
            };
            if DrawTextW(dc, &mut wide, &mut bounds, format | DT_CALCRECT) == 0 {
                return None;
            }
            let oversample = OVERSAMPLE as i32;
            // Whole output pixels, with the full width so centred lines stay centred.
            let width = ((max_width as i32 * oversample).max(bounds.right) + oversample - 1)
                / oversample
                * oversample;
            let height = (bounds.bottom + oversample - 1) / oversample * oversample;
            if width <= 0 || height <= 0 || width > MAX_SIDE || height > MAX_SIDE {
                return None;
            }

            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width,
                    // Negative: the first row is the top one.
                    biHeight: -height,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits: *mut c_void = std::ptr::null_mut();
            let bitmap =
                CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
            if bits.is_null() {
                let _ = DeleteObject(bitmap.into());
                return None;
            }
            canvas.select(bitmap.into());

            // White on the bitmap's black: the green channel is then the coverage.
            SetBkMode(dc, TRANSPARENT);
            SetTextColor(dc, COLORREF(0x00FF_FFFF));
            let mut area = RECT {
                left: 0,
                top: 0,
                right: width,
                bottom: height,
            };
            if DrawTextW(dc, &mut wide, &mut area, format) == 0 {
                return None;
            }
            let _ = GdiFlush();

            let pixels =
                std::slice::from_raw_parts(bits.cast::<u8>(), width as usize * height as usize * 4);
            let (out_width, out_height) =
                ((width / oversample) as u32, (height / oversample) as u32);
            let mut coverage = Vec::with_capacity((out_width * out_height) as usize);
            let step = OVERSAMPLE as usize;
            for row in 0..out_height as usize {
                for column in 0..out_width as usize {
                    let mut sum = 0u32;
                    for y in 0..step {
                        for x in 0..step {
                            let index =
                                ((row * step + y) * width as usize + column * step + x) * 4 + 1;
                            sum += u32::from(pixels[index]);
                        }
                    }
                    coverage.push((sum / (OVERSAMPLE * OVERSAMPLE)) as u8);
                }
            }
            Some(TextMask {
                width: out_width,
                height: out_height,
                coverage,
            })
        }
    }
}

#[cfg(not(windows))]
mod platform {
    use super::TextMask;

    pub(super) fn draw(_text: &str, _pixel_height: u32, _max_width: u32) -> Option<TextMask> {
        None
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn text_is_drawn_and_centred() {
        let mask = draw("We'll be right back", 56, 1600).expect("text is drawn");
        assert_eq!(mask.width, 1600);
        assert!((56..200).contains(&mask.height), "{}", mask.height);
        assert_eq!(mask.coverage.len(), (mask.width * mask.height) as usize);

        let columns: Vec<u32> = (0..mask.width)
            .map(|column| {
                (0..mask.height)
                    .map(|row| u32::from(mask.coverage[(row * mask.width + column) as usize]))
                    .sum()
            })
            .collect();
        let first = columns.iter().position(|&sum| sum > 0).unwrap() as i64;
        let last = columns.iter().rposition(|&sum| sum > 0).unwrap() as i64;
        // Ink somewhere solid, and the same margin on both sides, give or take a few pixels.
        assert!(mask.coverage.iter().any(|&value| value > 200));
        assert!((first - (i64::from(mask.width) - 1 - last)).abs() <= 6);
        assert!(last - first > 300);
    }

    #[test]
    fn long_text_wraps_and_blank_text_draws_nothing() {
        let one = draw("Back soon", 56, 600).unwrap();
        let wrapped = draw(
            "Back soon, after a much longer break than anyone planned for today",
            56,
            600,
        )
        .unwrap();
        assert!(wrapped.height > one.height);
        assert!(draw("   ", 56, 600).is_none());
    }
}
