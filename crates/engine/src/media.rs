//! Reads a video or still picture file for the away screen, through `native/media.c`.

use std::ffi::{CStr, CString, c_char, c_int};
use std::path::Path;
use std::ptr::NonNull;
use std::time::Duration;

use crate::decoder::{DecodeError, Nv12Planes, RawFrame, error_from_code};
use crate::picture::Rotation;

#[repr(C)]
struct RawMedia {
    _private: [u8; 0],
}

unsafe extern "C" {
    fn pc_media_open(path: *const c_char, error: *mut c_char, error_len: c_int) -> *mut RawMedia;
    fn pc_media_close(media: *mut RawMedia);
    fn pc_media_rotation(media: *const RawMedia) -> c_int;
    fn pc_media_next(media: *mut RawMedia, out: *mut RawFrame) -> c_int;
    fn pc_media_rewind(media: *mut RawMedia) -> c_int;
}

/// An open video or still picture file.
pub(crate) struct MediaFile {
    raw: NonNull<RawMedia>,
}

// SAFETY: the file owns all of its FFmpeg state and is only used from one thread at a time.
unsafe impl Send for MediaFile {}

/// One picture of a [`MediaFile`], borrowed until its next call.
pub(crate) struct MediaPicture<'a> {
    raw: RawFrame,
    _file: std::marker::PhantomData<&'a mut MediaFile>,
}

impl MediaPicture<'_> {
    /// When to show the picture, counted from the first one.
    pub(crate) fn time(&self) -> Duration {
        Duration::from_micros(self.raw.pts.max(0) as u64)
    }

    pub(crate) fn planes(&self) -> Nv12Planes<'_> {
        let height = self.raw.height as usize;
        // SAFETY: the C side guarantees `stride * rows` readable bytes in each plane until
        // the next call on the file, which the borrow rules out.
        unsafe {
            Nv12Planes {
                width: self.raw.width as u32,
                height: self.raw.height as u32,
                y: std::slice::from_raw_parts(self.raw.y, self.raw.y_stride as usize * height),
                y_stride: self.raw.y_stride as usize,
                uv: std::slice::from_raw_parts(
                    self.raw.uv,
                    self.raw.uv_stride as usize * (height / 2),
                ),
                uv_stride: self.raw.uv_stride as usize,
            }
        }
    }
}

impl MediaFile {
    pub(crate) fn open(path: &Path) -> Result<Self, DecodeError> {
        let text = path
            .to_str()
            .and_then(|text| CString::new(text).ok())
            .ok_or_else(|| DecodeError::new("the file name cannot be read"))?;
        let mut error = [0 as c_char; 256];
        // SAFETY: `text` is a terminated string and `error` is valid for its length.
        let raw = unsafe { pc_media_open(text.as_ptr(), error.as_mut_ptr(), error.len() as c_int) };
        NonNull::new(raw).map(|raw| Self { raw }).ok_or_else(|| {
            // SAFETY: the C side terminated the buffer.
            let text = unsafe { CStr::from_ptr(error.as_ptr()) };
            DecodeError::new(text.to_string_lossy())
        })
    }

    /// The turn the file asks for before it is shown, as phones record it.
    pub(crate) fn rotation(&self) -> Rotation {
        // SAFETY: `raw` is a live file.
        let degrees = unsafe { pc_media_rotation(self.raw.as_ptr()) };
        Rotation::from_degrees(degrees.clamp(0, 270) as u16).unwrap_or_default()
    }

    /// The next picture, or `None` at the end of the file.
    pub(crate) fn next(&mut self) -> Result<Option<MediaPicture<'_>>, DecodeError> {
        let mut raw = RawFrame::empty();
        // SAFETY: `raw` is a live file and `raw` frame is writable.
        let result = unsafe { pc_media_next(self.raw.as_ptr(), &mut raw) };
        match result {
            1 => Ok(Some(MediaPicture {
                raw,
                _file: std::marker::PhantomData,
            })),
            0 => Ok(None),
            code => Err(error_from_code(code)),
        }
    }

    /// Goes back to the first picture.
    pub(crate) fn rewind(&mut self) -> Result<(), DecodeError> {
        // SAFETY: `raw` is a live file.
        let result = unsafe { pc_media_rewind(self.raw.as_ptr()) };
        if result == 0 {
            Ok(())
        } else {
            Err(error_from_code(result))
        }
    }
}

impl Drop for MediaFile {
    fn drop(&mut self) {
        // SAFETY: `raw` came from `pc_media_open` and is closed exactly once.
        unsafe { pc_media_close(self.raw.as_ptr()) }
    }
}
