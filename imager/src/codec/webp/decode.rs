use image::{DynamicImage, GenericImage, GenericImageView, ImageBuffer, RgbaImage};
use libc::{c_float, size_t};
use libwebp_sys::WebPDecodeRGBA;
use std::ffi::{c_void, CString};
use std::os::raw::{c_char, c_int};

pub fn decode(source: &[u8]) -> DynamicImage {
    let mut width: i32 = 0;
    let mut height: i32 = 0;
    let decoded = unsafe { WebPDecodeRGBA(source.as_ptr(), source.len(), &mut width, &mut height) };
    assert!(!decoded.is_null());
    assert!(width != 0 && height != 0);
    let (width, height) = (width as u32, height as u32);
    let size = (width * height * 4) as usize;
    let output = unsafe { std::slice::from_raw_parts_mut(decoded, size).to_vec() };
    let media: RgbaImage = ImageBuffer::from_vec(width, height, output).expect("to ImageBuffer");
    let media = DynamicImage::ImageRgba8(media);
    media
}
