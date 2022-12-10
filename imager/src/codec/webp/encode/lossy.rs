use image::{DynamicImage, GenericImage, GenericImageView};
use libc::{c_float, size_t};
use libwebp_sys::{
    WebPConfig, WebPConfigInitInternal, WebPEncode, WebPMemoryWriter, WebPMemoryWriterClear,
    WebPPicture, WebPPictureFree, WebPPictureSharpARGBToYUVA, WebPPreset, WebPValidateConfig,
    WEBP_ENCODER_ABI_VERSION,
};
use std::ffi::{c_void, CString};
use std::os::raw::{c_char, c_int};

pub fn init_config(q: f32) -> WebPConfig {
    let mut config: WebPConfig = unsafe { std::mem::zeroed() };
    unsafe {
        WebPConfigInitInternal(
            &mut config,
            WebPPreset::WEBP_PRESET_DEFAULT,
            75.0,
            WEBP_ENCODER_ABI_VERSION as c_int,
        );
        WebPValidateConfig(&mut config);
    };
    config.quality = q;
    config.lossless = 0;
    config.method = 6;
    config
}

pub fn init_picture(source: &DynamicImage) -> (WebPPicture, *mut WebPMemoryWriter) {
    // SETUP
    let (mut picture, writer) = crate::codec::webp::encode::lossless::init_picture(source);
    // CONVERT
    unsafe {
        assert_ne!(WebPPictureSharpARGBToYUVA(&mut picture), 0);
        assert_eq!(picture.use_argb, 0);
        assert!(!picture.y.is_null());
    };
    // DONE
    (picture, writer)
}

pub fn encode(source: &DynamicImage, q: f32) -> Vec<u8> {
    let config = init_config(q);
    let (mut picture, writer_ptr) = init_picture(&source);
    unsafe {
        assert_ne!(WebPEncode(&config, &mut picture), 0);
    };
    // COPY OUTPUT
    let mut writer = unsafe { Box::from_raw(writer_ptr) };
    let mut output: Vec<u8> =
        unsafe { std::slice::from_raw_parts_mut(writer.mem, writer.size).to_vec() };
    // CLEANUP PICTURE & WRITER
    unsafe {
        WebPPictureFree(&mut picture);
        WebPMemoryWriterClear(writer_ptr);
        std::mem::drop(picture);
        std::mem::drop(writer_ptr);
        std::mem::drop(writer);
    };
    // DONE
    output
}
