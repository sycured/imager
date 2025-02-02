// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use image::{DynamicImage, GenericImage, GenericImageView, ImageBuffer, ImageFormat};
use itertools::Itertools;
use libc::{c_float, c_void, size_t};
use libwebp_sys::{WebPConfig, WebPMemoryWriter, WebPPicture, WEBP_MAX_DIMENSION};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::LinkedList;
use std::convert::{AsRef, TryFrom};
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::str::FromStr;
use std::sync::Arc;

///////////////////////////////////////////////////////////////////////////////
// OUTPUT-FORMAT
///////////////////////////////////////////////////////////////////////////////

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum OutputFormat {
    Jpeg,
    Png,
    Webp,
}

impl OutputFormat {
    pub fn infer_from_file_container<P: AsRef<Path>>(path: P) -> Option<Self> {
        let buffer = std::fs::read(path).ok()?;
        let format = ::image::guess_format(&buffer).ok()?;
        match format {
            ImageFormat::Jpeg => Some(Self::Jpeg),
            ImageFormat::Png => Some(Self::Png),
            ImageFormat::WebP => Some(Self::Webp),
            _ => None,
        }
    }
    pub fn infer_from_path<P: AsRef<Path>>(path: P) -> Option<Self> {
        let ext = path.as_ref().extension()?.to_str()?;
        Self::from_str(ext).ok()
    }
}

impl FromStr for OutputFormat {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "jpeg" => Ok(Self::Jpeg),
            "jpg" => Ok(Self::Jpeg),
            "png" => Ok(Self::Png),
            "webp" => Ok(Self::Webp),
            _ => Err(format!("Unknown or unsupported output format {}", s)),
        }
    }
}

impl Default for OutputFormat {
    fn default() -> Self {
        Self::Jpeg
    }
}

#[derive(Debug, Clone)]
pub struct OutputFormats(pub Vec<OutputFormat>);

impl Default for OutputFormats {
    fn default() -> Self {
        OutputFormats(vec![OutputFormat::Jpeg, OutputFormat::Webp])
    }
}

impl FromStr for OutputFormats {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut invalids = Vec::new();
        let results = s
            .split_whitespace()
            .filter_map(|x| match OutputFormat::from_str(x) {
                Ok(x) => Some(x),
                Err(e) => {
                    invalids.push(e);
                    None
                }
            })
            .collect::<Vec<_>>();
        if invalids.is_empty() {
            Ok(Self(results))
        } else {
            Err(invalids.join(", "))
        }
    }
}

///////////////////////////////////////////////////////////////////////////////
// RESOLUTION
///////////////////////////////////////////////////////////////////////////////

#[derive(Debug, PartialEq, Clone, Serialize, Deserialize, Eq)]
pub struct Resolution {
    pub width: u32,
    pub height: u32,
}

impl Resolution {
    #[must_use] pub fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }
}

impl std::fmt::Display for Resolution {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}x{}", self.width, self.height)
    }
}

impl FromStr for Resolution {
    type Err = String;
    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let ix = input.find("x").ok_or("invalid")?;
        let (width, height) = input.split_at(ix);
        let height = height.trim_start_matches("x");
        let width = u32::from_str(width).map_err(|_| "invalid")?;
        let height = u32::from_str(height).map_err(|_| "invalid")?;
        Ok(Self { width, height })
    }
}

///////////////////////////////////////////////////////////////////////////////
// OUTPUT-SIZE
///////////////////////////////////////////////////////////////////////////////

#[derive(Debug, Clone, PartialEq)]
pub enum OutputSize {
    /// Output image resolution. Akin to the 'px' CSS unit.
    Px(Resolution),
    /// Retain the original resolution. Akin to the '100%' CSS value.
    Full,
}

impl std::fmt::Display for OutputSize {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Px(px) => write!(f, "{}", px),
            Self::Full => write!(f, "full"),
        }
    }
}

impl FromStr for OutputSize {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s == "full" { Ok(Self::Full) } else {
            let val: Resolution = Resolution::from_str(s)?;
            Ok(Self::Px(val))
        }
    }
}

impl Default for OutputSize {
    fn default() -> Self {
        Self::Full
    }
}

impl Serialize for OutputSize {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for OutputSize {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

///////////////////////////////////////////////////////////////////////////////
// MISC HELPERS
///////////////////////////////////////////////////////////////////////////////

#[must_use] pub fn ensure_even_reslution(source: &DynamicImage) -> DynamicImage {
    let (width, height) = source.dimensions();
    // ENSURE EVEN
    let even_width = (width % 2) == 0;
    let even_height = (height % 2) == 0;
    if (!even_width) || (!even_height) {
        let new_width = {
            if !even_width {
                width - 1
            } else {
                width
            }
        };
        let new_height = {
            if !even_height {
                height - 1
            } else {
                height
            }
        };
        source.clone().crop(0, 0, new_width, new_height)
    } else {
        source.clone()
    }
}

///////////////////////////////////////////////////////////////////////////////
// INTERNAL HELPERS
///////////////////////////////////////////////////////////////////////////////

pub fn open_dir_sorted_paths<P: AsRef<Path>>(path: P) -> Vec<PathBuf> {
    std::fs::read_dir(path)
        .expect("read source dir")
        .filter_map(Result::ok)
        .filter(|x| x.file_type().expect("source filetype").is_file())
        .map(|x| x.path())
        .filter_map(|x| {
            let file_name = x
                .file_name()
                .expect("missing file name")
                .to_str()
                .expect("file name to str")
                .chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>();
            let index = file_name.parse::<usize>().ok()?;
            Some((index, x))
        })
        .sorted_by(|(i, _), (j, _)| i.cmp(j))
        .map(|(_, x)| x)
        .collect::<Vec<_>>()
}

unsafe fn convert_to_yuv_using_webp(source: &DynamicImage) -> Yuv420P {
    // ENSURE IMAGE IS EVEN
    let source = ensure_even_reslution(source);
    let (width, height) = source.dimensions();
    // WEBP INVARIANTS
    assert!(width < WEBP_MAX_DIMENSION);
    assert!(height < WEBP_MAX_DIMENSION);
    // INIT WEBP
    let mut picture: WebPPicture = unsafe { std::mem::zeroed() };
    unsafe {
        assert_ne!(libwebp_sys::WebPPictureInit(&mut picture), false);
    };
    let argb_stride = width;
    picture.use_argb = 1;
    picture.width = width as i32;
    picture.height = height as i32;
    picture.argb_stride = argb_stride as i32;
    // FILL PIXEL BUFFERS
    unsafe {
        let mut pixel_data = source
            .to_rgb8()
            .pixels()
            .flat_map(|px: &::image::Rgb<u8>| px.0.to_vec())
            .collect::<Vec<_>>();
        let full_stride = argb_stride * 3;
        let status = libwebp_sys::WebPPictureImportRGB(
            &mut picture,
            pixel_data.as_mut_ptr(),
            full_stride as i32,
        );
        // CHECKS
        let expected_size = argb_stride * height * 3;
        assert_eq!(pixel_data.len() as u32, expected_size);
        assert_ne!(status, 0);
        // CLEANUP
        std::mem::drop(pixel_data);
    };
    // CHECKS
    assert_eq!(picture.use_argb, 1);
    assert!(picture.y.is_null());
    assert!(!picture.argb.is_null());
    // CONVERT
    unsafe {
        assert_ne!(libwebp_sys::WebPPictureSharpARGBToYUVA(&mut picture), 0);
        assert_eq!(picture.use_argb, 0);
        assert!(!picture.y.is_null());
    };
    let data = unsafe {
        assert_eq!(picture.y_stride as u32, width);
        assert_eq!(picture.uv_stride as u32, width / 2);
        let y_size = width * height;
        let uv_size = width * height / 4;
        let y = std::slice::from_raw_parts_mut(picture.y, y_size as usize).to_vec();
        let u = std::slice::from_raw_parts_mut(picture.u, uv_size as usize).to_vec();
        let v = std::slice::from_raw_parts_mut(picture.v, uv_size as usize).to_vec();
        [y, u, v].concat()
    };
    // CLEANUP
    unsafe {
        libwebp_sys::WebPPictureFree(&mut picture);
    };
    std::mem::drop(picture);
    // DONE
    let result = Yuv420P { width, height, data };
    assert!(result.expected_yuv420p_size());
    result
}

unsafe fn convert_to_rgba_using_webp(source: &Yuv420P) -> DynamicImage {
    let (width, height) = source.dimensions();
    assert!(width < WEBP_MAX_DIMENSION);
    assert!(height < WEBP_MAX_DIMENSION);
    let mut picture: WebPPicture = unsafe { std::mem::zeroed() };
    assert_ne!(libwebp_sys::WebPPictureInit(&mut picture), false);
    let argb_stride = width;
    picture.use_argb = 0;
    picture.width = width as i32;
    picture.height = height as i32;
    picture.argb_stride = argb_stride as i32;
    picture.colorspace = libwebp_sys::WebPEncCSP::WEBP_YUV420;
    // ALLOCATE
    assert_ne!(libwebp_sys::WebPPictureAlloc(&mut picture), 0);
    // FILL SOURCE PIXEL BUFFERS
    {
        // CHECKS
        assert!(!picture.y.is_null());
        assert!(!picture.u.is_null());
        assert!(!picture.v.is_null());
        // GO
        let y_size = source.luma_size();
        let uv_size = source.chroma_size();
        let mut y = std::slice::from_raw_parts_mut(picture.y, y_size as usize);
        let mut u = std::slice::from_raw_parts_mut(picture.u, uv_size as usize);
        let mut v = std::slice::from_raw_parts_mut(picture.v, uv_size as usize);
        y.copy_from_slice(source.y());
        u.copy_from_slice(source.u());
        v.copy_from_slice(source.v());
    };
    // CONVERT
    assert!(picture.argb.is_null());
    assert_eq!(libwebp_sys::WebPPictureHasTransparency(&picture), 0);
    assert_ne!(libwebp_sys::WebPPictureYUVAToARGB(&mut picture,), 0);
    // CHECKS
    assert_eq!(picture.use_argb, 1);
    assert!(!picture.argb.is_null());
    assert_eq!(libwebp_sys::WebPPictureHasTransparency(&picture), 0);
    // GET RESULT DATA
    assert_eq!(picture.argb_stride as u32, width);
    let rgba_output = ::image::RgbaImage::from_fn(width, height, |x_pos, y_pos| {
        let ptr_ix = (y_pos * width) + x_pos;
        let px = *picture.argb.add(ptr_ix as usize);
        let [a, r, g, b]: [u8; 4] = px.to_be().to_ne_bytes();
        ::image::Rgba([r, g, b, a])
    });
    let rgba_output = DynamicImage::ImageRgba8(rgba_output);
    // CLEANUP
    unsafe {
        libwebp_sys::WebPPictureFree(&mut picture);
    };
    std::mem::drop(picture);
    // DONE
    rgba_output
}

///////////////////////////////////////////////////////////////////////////////
// PICTURE BUFFERS
///////////////////////////////////////////////////////////////////////////////

#[derive(Debug, Clone)]
pub struct Yuv420P {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

impl Yuv420P {
    pub fn open_image<P: AsRef<Path>>(path: P) -> Result<Self, ()> {
        let source = ::image::open(path).expect("Yuv420P::open_image - load image");
        Self::from_image(&source)
    }
    pub fn from_image(source: &DynamicImage) -> Result<Self, ()> {
        Ok(unsafe { convert_to_yuv_using_webp(source) })
    }
    pub fn open_yuv<P: AsRef<Path>>(path: P, width: u32, height: u32) -> Result<Self, ()> {
        let source = std::fs::read(path).expect("read raw yuv file");
        let result = Self {
            width,
            height,
            data: source,
        };
        assert!(result.expected_yuv420p_size());
        Ok(result)
    }
    #[must_use]
    pub fn luma_size(&self) -> u32 {
        self.width * self.height
    }
    #[must_use]
    pub fn chroma_size(&self) -> u32 {
        self.width * self.height / 4
    }
    #[must_use]
    pub fn expected_yuv420p_size(&self) -> bool {
        let expected_size = {
            let l = self.luma_size();
            let c = self.chroma_size();
            l + c + c
        };
        self.data.len() == (expected_size as usize)
    }
    pub fn save(&self, path: &str) {
        println!(
            "ffplay -video_size {}x{} -pixel_format yuv420p {}",
            self.width, self.height, path,
        );
        std::fs::write(path, &self.data);
    }
    #[must_use]
    pub fn to_rgba_image(&self) -> DynamicImage {
        unsafe { convert_to_rgba_using_webp(self) }
    }
    #[must_use]
    pub fn y(&self) -> &[u8] {
        assert!(self.expected_yuv420p_size());
        let end = self.luma_size();
        self.data.get(0..end as usize).expect("bad (Y) plane size")
    }
    #[must_use]
    pub fn u(&self) -> &[u8] {
        assert!(self.expected_yuv420p_size());
        let plane = self
            .data
            .as_slice()
            .split_at(self.luma_size() as usize)
            .1
            .chunks(self.chroma_size() as usize)
            .next()
            .expect("bad (U) plane chunk size");
        assert_eq!(plane.len(), self.chroma_size() as usize);
        plane
    }
    #[must_use]
    pub fn v(&self) -> &[u8] {
        assert!(self.expected_yuv420p_size());
        let plane = self
            .data
            .as_slice()
            .split_at(self.luma_size() as usize)
            .1
            .chunks(self.chroma_size() as usize)
            .nth(1)
            .expect("bad (V) plane chunk size");
        assert_eq!(plane.len(), self.chroma_size() as usize);
        plane
    }
    #[must_use]
    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }
}

///////////////////////////////////////////////////////////////////////////////
// VIDEO FRAME BUFFERS
///////////////////////////////////////////////////////////////////////////////

#[derive(Debug, Clone)]
pub struct VideoBuffer {
    width: u32,
    height: u32,
    frames: Arc<Vec<Yuv420P>>,
    cursor: usize,
}

impl VideoBuffer {
    pub fn from_png(source: &[u8]) -> Result<Self, ()> {
        let source = ::image::load_from_memory_with_format(source, ::image::ImageFormat::Png);
        let source = source.expect("load png source");
        Self::from_image(&source)
    }
    pub fn from_jpeg(source: &[u8]) -> Result<Self, ()> {
        let source = ::image::load_from_memory_with_format(source, ::image::ImageFormat::Jpeg);
        let source = source.expect("load jpeg source");
        Self::from_image(&source)
    }
    pub fn from_image(source: &DynamicImage) -> Result<Self, ()> {
        Ok(Self::singleton(Yuv420P::from_image(source)?))
    }
    #[must_use] pub fn singleton(frame: Yuv420P) -> Self {
        Self {
            width: frame.width,
            height: frame.height,
            frames: Arc::new(vec![frame]),
            cursor: 0,
        }
    }
    pub fn open_image_dir<P: AsRef<Path>>(dir_path: P) -> Result<Self, ()> {
        assert!(dir_path.as_ref().exists());
        let frames = open_dir_sorted_paths(dir_path)
            .into_par_iter()
            .map(|path| Yuv420P::open_image(&path).expect("open and decode image"))
            .collect::<Vec<_>>();
        assert!(!frames.is_empty());
        let (width, height) = {
            let w = frames[0].width;
            let h = frames[0].height;
            (w, h)
        };
        Ok(VideoBuffer {
            width,
            height,
            frames: Arc::new(frames),
            cursor: 0,
        })
    }
    #[must_use] pub fn width(&self) -> u32 {
        self.width
    }
    #[must_use] pub fn height(&self) -> u32 {
        self.height
    }
    #[must_use] pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }
    #[must_use] pub fn as_frames(&self) -> &[Yuv420P] {
        self.frames.as_ref()
    }
    #[must_use] pub fn into_frames(self) -> Vec<Yuv420P> {
        let refs = Arc::strong_count(&self.frames);
        if refs == 0 {
            Arc::try_unwrap(self.frames).expect("shuld have no other refs")
        } else {
            self.frames.as_ref().clone()
        }
    }
    pub fn next(&mut self) -> Option<&Yuv420P> {
        let frame = self.frames.get(self.cursor)?;
        self.cursor += 1;
        Some(frame)
    }
    pub fn set_cursor(&mut self, cursor_pos: usize) {
        self.cursor = cursor_pos;
    }
    #[must_use] pub fn position(&self) -> usize {
        self.cursor
    }
    #[must_use] pub fn as_fresh_cursor(&self) -> VideoBuffer {
        VideoBuffer {
            width: self.width,
            height: self.height,
            frames: self.frames.clone(),
            cursor: self.cursor,
        }
    }
}
