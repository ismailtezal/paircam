//! Safe wrappers over the C decoding layer in `native/decoder.c`.

use std::ffi::{CStr, c_char, c_int};
use std::marker::PhantomData;
use std::ptr::NonNull;

use paircam_protocol::Codec;

use crate::colour::ColourTables;
use crate::picture::{PixelRect, Plan};

#[repr(C)]
struct RawFrame {
    width: i32,
    height: i32,
    pts: i64,
    y: *const u8,
    y_stride: i32,
    uv: *const u8,
    uv_stride: i32,
}

/// Mirrors `PcTransform` in `native/decoder.c`.
#[repr(C)]
struct RawTransform {
    crop_x: i32,
    crop_y: i32,
    crop_width: i32,
    crop_height: i32,
    rotation: i32,
    mirror: i32,
    flip: i32,
    content_x: i32,
    content_y: i32,
    content_width: i32,
    content_height: i32,
}

#[repr(C)]
struct RawDecoder {
    _private: [u8; 0],
}

#[repr(C)]
struct RawScaler {
    _private: [u8; 0],
}

unsafe extern "C" {
    fn pc_decoder_open(
        codec: c_int,
        record: *const u8,
        record_len: c_int,
        use_hardware: c_int,
        error: *mut c_char,
        error_len: c_int,
    ) -> *mut RawDecoder;
    fn pc_decoder_close(decoder: *mut RawDecoder);
    fn pc_decoder_is_hardware(decoder: *const RawDecoder) -> c_int;
    fn pc_decoder_decode(
        decoder: *mut RawDecoder,
        data: *const u8,
        len: c_int,
        pts: i64,
        out: *mut RawFrame,
    ) -> c_int;
    fn pc_error_string(code: c_int, error: *mut c_char, error_len: c_int);

    fn pc_scaler_new() -> *mut RawScaler;
    fn pc_scaler_free(scaler: *mut RawScaler);
    fn pc_scaler_nv12_to_bgra(
        scaler: *mut RawScaler,
        frame: *const RawFrame,
        bgra: *mut u8,
        bgra_width: c_int,
        bgra_height: c_int,
        bgra_stride: c_int,
    ) -> c_int;
    #[allow(clippy::too_many_arguments)]
    fn pc_scaler_nv12_to_nv12(
        scaler: *mut RawScaler,
        frame: *const RawFrame,
        y: *mut u8,
        y_stride: c_int,
        uv: *mut u8,
        uv_stride: c_int,
        width: c_int,
        height: c_int,
    ) -> c_int;
    #[allow(clippy::too_many_arguments)]
    fn pc_transform_nv12(
        scaler: *mut RawScaler,
        frame: *const RawFrame,
        transform: *const RawTransform,
        y: *mut u8,
        y_stride: c_int,
        uv: *mut u8,
        uv_stride: c_int,
        width: c_int,
        height: c_int,
    ) -> c_int;
    fn pc_colour_nv12(
        colour: *const ColourTables,
        y: *mut u8,
        y_stride: c_int,
        uv: *mut u8,
        uv_stride: c_int,
        width: c_int,
        height: c_int,
    ) -> c_int;
}

/// Grades the `area` of `target` in place with `tables`. Every edge of the area must be even
/// and inside the picture; the rest of the picture, such as black bars, is left alone.
pub fn apply_colour(
    tables: &ColourTables,
    target: Nv12PlanesMut<'_>,
    area: PixelRect,
) -> Result<(), DecodeError> {
    let even = |value: u32| value.is_multiple_of(2);
    let inside = area
        .x
        .checked_add(area.width)
        .is_some_and(|right| right <= target.width)
        && area
            .y
            .checked_add(area.height)
            .is_some_and(|bottom| bottom <= target.height);
    if !target.as_planes().is_valid()
        || !inside
        || !even(area.x)
        || !even(area.y)
        || !even(area.width)
        || !even(area.height)
    {
        return Err(DecodeError(
            "the area to grade is not inside the picture".into(),
        ));
    }
    if area.width == 0 || area.height == 0 {
        return Ok(());
    }
    let y_start = area.y as usize * target.y_stride + area.x as usize;
    let uv_start = (area.y / 2) as usize * target.uv_stride + area.x as usize;
    // SAFETY: the planes were checked to hold the whole picture and the area lies inside it,
    // so every row the C side touches is within the exclusive slices borrowed for the call.
    let result = unsafe {
        pc_colour_nv12(
            tables,
            target.y.as_mut_ptr().add(y_start),
            target.y_stride as c_int,
            target.uv.as_mut_ptr().add(uv_start),
            target.uv_stride as c_int,
            area.width as c_int,
            area.height as c_int,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(error_from_code(result))
    }
}

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct DecodeError(String);

fn error_from_code(code: c_int) -> DecodeError {
    let mut buffer = [0 as c_char; 128];
    // SAFETY: the buffer is valid for its length and the C side always terminates it.
    let text = unsafe {
        pc_error_string(code, buffer.as_mut_ptr(), buffer.len() as c_int);
        CStr::from_ptr(buffer.as_ptr())
    };
    DecodeError(text.to_string_lossy().into_owned())
}

/// A decoded picture in NV12 layout, borrowed from the decoder until its next call.
pub struct Nv12Frame<'a> {
    raw: RawFrame,
    _decoder: PhantomData<&'a mut Decoder>,
}

impl Nv12Frame<'_> {
    pub fn width(&self) -> u32 {
        self.raw.width as u32
    }

    pub fn height(&self) -> u32 {
        self.raw.height as u32
    }

    /// The timestamp passed in with the access unit this picture came from.
    pub fn timestamp_us(&self) -> u64 {
        self.raw.pts as u64
    }

    /// The luma plane: `height` rows of `y_stride` bytes.
    pub fn y(&self) -> &[u8] {
        // SAFETY: the decoder guarantees `y_stride * height` readable bytes while borrowed.
        unsafe {
            std::slice::from_raw_parts(
                self.raw.y,
                self.raw.y_stride as usize * self.height() as usize,
            )
        }
    }

    pub fn y_stride(&self) -> usize {
        self.raw.y_stride as usize
    }

    /// The interleaved chroma plane: `height / 2` rows of `uv_stride` bytes.
    pub fn uv(&self) -> &[u8] {
        let rows = self.height().div_ceil(2) as usize;
        // SAFETY: the decoder guarantees `uv_stride * rows` readable bytes while borrowed.
        unsafe { std::slice::from_raw_parts(self.raw.uv, self.raw.uv_stride as usize * rows) }
    }

    pub fn uv_stride(&self) -> usize {
        self.raw.uv_stride as usize
    }

    /// Copies the picture into `target` as tightly packed NV12 (`width * height * 3 / 2` bytes
    /// for even sizes), which is what the virtual camera consumes.
    pub fn copy_packed(&self, target: &mut Vec<u8>) {
        let width = self.width() as usize;
        let height = self.height() as usize;
        let chroma_rows = height.div_ceil(2);
        let chroma_width = width.div_ceil(2) * 2;

        target.clear();
        target.reserve(width * height + chroma_width * chroma_rows);
        for row in self.y().chunks_exact(self.y_stride()).take(height) {
            target.extend_from_slice(&row[..width]);
        }
        for row in self.uv().chunks_exact(self.uv_stride()).take(chroma_rows) {
            target.extend_from_slice(&row[..chroma_width]);
        }
    }
}

/// Decodes H.264 or HEVC into NV12, on the GPU when it can.
pub struct Decoder {
    raw: NonNull<RawDecoder>,
}

// SAFETY: the decoder owns all of its FFmpeg state and is only used from one thread at a time.
unsafe impl Send for Decoder {}

impl Decoder {
    /// Opens a decoder for a stream described by its `avcC` or `hvcC` configuration record.
    pub fn new(codec: Codec, record: &[u8], use_hardware: bool) -> Result<Self, DecodeError> {
        let mut error = [0 as c_char; 256];
        // SAFETY: `record` and `error` are valid for the lengths passed.
        let raw = unsafe {
            pc_decoder_open(
                match codec {
                    Codec::H264 => 0,
                    Codec::Hevc => 1,
                },
                record.as_ptr(),
                record.len() as c_int,
                c_int::from(use_hardware),
                error.as_mut_ptr(),
                error.len() as c_int,
            )
        };
        NonNull::new(raw).map(|raw| Self { raw }).ok_or_else(|| {
            // SAFETY: the C side terminated the buffer.
            let text = unsafe { CStr::from_ptr(error.as_ptr()) };
            DecodeError(text.to_string_lossy().into_owned())
        })
    }

    /// Whether decoding runs on the GPU. This can turn false after the first frame if the GPU
    /// turns out not to support the stream.
    pub fn is_hardware(&self) -> bool {
        // SAFETY: `raw` is a live decoder.
        unsafe { pc_decoder_is_hardware(self.raw.as_ptr()) != 0 }
    }

    /// Decodes one access unit (NAL units, each preceded by its `u32` length).
    ///
    /// Returns `None` when the decoder needs more data before it can output a picture.
    pub fn decode(
        &mut self,
        access_unit: &[u8],
        timestamp_us: u64,
    ) -> Result<Option<Nv12Frame<'_>>, DecodeError> {
        let mut raw = RawFrame {
            width: 0,
            height: 0,
            pts: 0,
            y: std::ptr::null(),
            y_stride: 0,
            uv: std::ptr::null(),
            uv_stride: 0,
        };
        // SAFETY: `raw` is a live decoder and the slice is valid for its length.
        let result = unsafe {
            pc_decoder_decode(
                self.raw.as_ptr(),
                access_unit.as_ptr(),
                access_unit.len() as c_int,
                timestamp_us as i64,
                &mut raw,
            )
        };
        match result {
            1 => Ok(Some(Nv12Frame {
                raw,
                _decoder: PhantomData,
            })),
            0 => Ok(None),
            code => Err(error_from_code(code)),
        }
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        // SAFETY: `raw` came from `pc_decoder_open` and is closed exactly once.
        unsafe { pc_decoder_close(self.raw.as_ptr()) }
    }
}

/// Converts NV12 pictures to BGRA at any size, for the preview.
pub struct Scaler {
    raw: NonNull<RawScaler>,
}

// SAFETY: the scaler owns its state and is only used from one thread at a time.
unsafe impl Send for Scaler {}

impl Scaler {
    pub fn new() -> Self {
        // SAFETY: allocation only; a null result means the process is out of memory.
        let raw = unsafe { pc_scaler_new() };
        Self {
            raw: NonNull::new(raw).expect("out of memory"),
        }
    }

    /// Scales `frame` to `width` by `height` and writes tightly packed BGRA into `target`.
    pub fn nv12_to_bgra(
        &mut self,
        frame: &Nv12Frame<'_>,
        width: u32,
        height: u32,
        target: &mut Vec<u8>,
    ) -> Result<(), DecodeError> {
        self.raw_to_bgra(&frame.raw, width, height, target)
    }

    /// Like [`Scaler::nv12_to_bgra`], for an NV12 picture in memory.
    pub fn planes_to_bgra(
        &mut self,
        source: Nv12Planes<'_>,
        width: u32,
        height: u32,
        target: &mut Vec<u8>,
    ) -> Result<(), DecodeError> {
        if !source.is_valid() {
            return Err(DecodeError(
                "NV12 planes are too small for their size".into(),
            ));
        }
        self.raw_to_bgra(&source.raw(), width, height, target)
    }

    fn raw_to_bgra(
        &mut self,
        frame: &RawFrame,
        width: u32,
        height: u32,
        target: &mut Vec<u8>,
    ) -> Result<(), DecodeError> {
        let stride = width as usize * 4;
        target.resize(stride * height as usize, 0);
        // SAFETY: `target` holds `stride * height` bytes and the frame planes are live.
        let result = unsafe {
            pc_scaler_nv12_to_bgra(
                self.raw.as_ptr(),
                frame,
                target.as_mut_ptr(),
                width as c_int,
                height as c_int,
                stride as c_int,
            )
        };
        if result == 0 {
            Ok(())
        } else {
            Err(error_from_code(result))
        }
    }

    /// Turns `source` into `target` as `plan` says: the crop, scaled into the content
    /// rectangle, turned, mirrored and flipped, with black bars around it. The target's size
    /// must be the plan's output size.
    ///
    /// Like [`Scaler::nv12_to_nv12`], keep one scaler per target size.
    pub fn transform_nv12(
        &mut self,
        source: Nv12Planes<'_>,
        plan: &Plan,
        target: Nv12PlanesMut<'_>,
    ) -> Result<(), DecodeError> {
        if !source.is_valid() || !target.as_planes().is_valid() {
            return Err(DecodeError(
                "NV12 planes are too small for their size".into(),
            ));
        }
        if (target.width, target.height) != (plan.output_width, plan.output_height) {
            return Err(DecodeError("the target does not match the plan".into()));
        }
        let int = |value: u32| value.min(i32::MAX as u32) as i32;
        let transform = RawTransform {
            crop_x: int(plan.crop.x),
            crop_y: int(plan.crop.y),
            crop_width: int(plan.crop.width),
            crop_height: int(plan.crop.height),
            rotation: i32::from(plan.rotation.degrees()),
            mirror: i32::from(plan.mirror),
            flip: i32::from(plan.flip),
            content_x: int(plan.content.x),
            content_y: int(plan.content.y),
            content_width: int(plan.content.width),
            content_height: int(plan.content.height),
        };
        // SAFETY: both pictures were checked to fit their slices, which stay borrowed for the
        // call, and the target slices are exclusive. The C side checks the rectangles against
        // the sizes before touching any pixel.
        let result = unsafe {
            pc_transform_nv12(
                self.raw.as_ptr(),
                &source.raw(),
                &transform,
                target.y.as_mut_ptr(),
                target.y_stride as c_int,
                target.uv.as_mut_ptr(),
                target.uv_stride as c_int,
                target.width as c_int,
                target.height as c_int,
            )
        };
        if result == 0 {
            Ok(())
        } else {
            Err(error_from_code(result))
        }
    }

    /// Scales an NV12 picture in memory into `target`, at the target's size. The target planes
    /// may be a window inside a larger picture, which is how the virtual camera letterboxes.
    ///
    /// Use a separate scaler from the one used for [`Scaler::nv12_to_bgra`], or the FFmpeg
    /// context is rebuilt on every switch.
    pub fn nv12_to_nv12(
        &mut self,
        source: Nv12Planes<'_>,
        target: Nv12PlanesMut<'_>,
    ) -> Result<(), DecodeError> {
        if !source.is_valid() || !target.as_planes().is_valid() {
            return Err(DecodeError(
                "NV12 planes are too small for their size".into(),
            ));
        }
        // SAFETY: both pictures were checked to fit their slices, which stay borrowed for the
        // call, and the target slices are exclusive.
        let result = unsafe {
            pc_scaler_nv12_to_nv12(
                self.raw.as_ptr(),
                &source.raw(),
                target.y.as_mut_ptr(),
                target.y_stride as c_int,
                target.uv.as_mut_ptr(),
                target.uv_stride as c_int,
                target.width as c_int,
                target.height as c_int,
            )
        };
        if result == 0 {
            Ok(())
        } else {
            Err(error_from_code(result))
        }
    }
}

/// An NV12 picture in memory: a luma plane and an interleaved chroma plane, each with a stride.
#[derive(Clone, Copy)]
pub struct Nv12Planes<'a> {
    pub width: u32,
    pub height: u32,
    pub y: &'a [u8],
    pub y_stride: usize,
    pub uv: &'a [u8],
    pub uv_stride: usize,
}

/// A writable [`Nv12Planes`].
pub struct Nv12PlanesMut<'a> {
    pub width: u32,
    pub height: u32,
    pub y: &'a mut [u8],
    pub y_stride: usize,
    pub uv: &'a mut [u8],
    pub uv_stride: usize,
}

impl Nv12PlanesMut<'_> {
    fn as_planes(&self) -> Nv12Planes<'_> {
        Nv12Planes {
            width: self.width,
            height: self.height,
            y: self.y,
            y_stride: self.y_stride,
            uv: self.uv,
            uv_stride: self.uv_stride,
        }
    }
}

impl Nv12Planes<'_> {
    /// The C view of the planes. Only valid while the borrowed slices are.
    fn raw(&self) -> RawFrame {
        RawFrame {
            width: self.width as i32,
            height: self.height as i32,
            pts: 0,
            y: self.y.as_ptr(),
            y_stride: self.y_stride as i32,
            uv: self.uv.as_ptr(),
            uv_stride: self.uv_stride as i32,
        }
    }

    /// Whether both planes hold every row the size calls for, and the size fits FFmpeg.
    fn is_valid(&self) -> bool {
        fn fits(len: usize, stride: usize, rows: usize, row_bytes: usize) -> bool {
            stride >= row_bytes && len >= stride * (rows - 1) + row_bytes
        }
        let width = self.width as usize;
        let height = self.height as usize;
        width > 0
            && height > 0
            && self.width <= i32::MAX as u32
            && self.height <= i32::MAX as u32
            && self.y_stride <= i32::MAX as usize
            && self.uv_stride <= i32::MAX as usize
            && fits(self.y.len(), self.y_stride, height, width)
            && fits(
                self.uv.len(),
                self.uv_stride,
                height.div_ceil(2),
                width.div_ceil(2) * 2,
            )
    }
}

impl Default for Scaler {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Scaler {
    fn drop(&mut self) {
        // SAFETY: `raw` came from `pc_scaler_new` and is freed exactly once.
        unsafe { pc_scaler_free(self.raw.as_ptr()) }
    }
}
