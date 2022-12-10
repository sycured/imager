use image::{DynamicImage, GenericImage, GenericImageView};
use libc::{c_float, size_t};
use libwebp_sys::{
    WebPConfig, WebPConfigInitInternal, WebPEncode, WebPMemoryWrite, WebPMemoryWriter,
    WebPMemoryWriterClear, WebPMemoryWriterInit, WebPPicture, WebPPictureFree,
    WebPPictureImportRGBA, WebPPictureInit, WebPPreset, WebPValidateConfig,
    WEBP_ENCODER_ABI_VERSION, WEBP_MAX_DIMENSION,
};
use std::ffi::{c_void, CString};
use std::os::raw::{c_char, c_int};

pub fn init_config() -> WebPConfig {
    let mut config: WebPConfig = unsafe { std::mem::zeroed() };
    unsafe {
        // webp_sys::webp_config_init(&mut config);
        WebPConfigInitInternal(
            &mut config,
            WebPPreset::WEBP_PRESET_DEFAULT,
            75.0,
            WEBP_ENCODER_ABI_VERSION as c_int,
        );

        WebPValidateConfig(&mut config);
    };
    config.lossless = 1;
    config.quality = 100.0;
    config.method = 6;
    config
}

pub fn init_picture(source: &DynamicImage) -> (WebPPicture, *mut WebPMemoryWriter) {
    let (width, height) = source.dimensions();
    assert!(width < WEBP_MAX_DIMENSION);
    assert!(height < WEBP_MAX_DIMENSION);
    let mut picture: WebPPicture = unsafe { std::mem::zeroed() };
    unsafe {
        assert_ne!(WebPPictureInit(&mut picture), false);
    };
    let argb_stride = width;
    picture.use_argb = 1;
    picture.width = width as i32;
    picture.height = height as i32;
    picture.argb_stride = argb_stride as i32;
    // FILL PIXEL BUFFERS
    unsafe {
        let mut pixel_data = source
            .to_rgba8()
            .pixels()
            .flat_map(|px| px.0.to_vec())
            .collect::<Vec<_>>();
        let full_stride = argb_stride * 4;
        let status =
            WebPPictureImportRGBA(&mut picture, pixel_data.as_mut_ptr(), full_stride as i32);
        // CHECKS
        let expected_size = argb_stride * height * 4;
        assert_eq!(pixel_data.len() as u32, expected_size);
        assert_ne!(status, 0);
        // CLEANUP
        std::mem::drop(pixel_data);
    };
    // CHECKS
    assert_eq!(picture.use_argb, 1);
    assert!(picture.y.is_null());
    assert!(!picture.argb.is_null());
    // OUTPUT WRITER
    let mut writer = unsafe {
        let mut writer: WebPMemoryWriter = std::mem::zeroed();
        WebPMemoryWriterInit(&mut writer);
        Box::into_raw(Box::new(writer))
    };
    unsafe extern "C" fn on_write(
        data: *const u8,
        data_size: usize,
        picture: *const WebPPicture,
    ) -> c_int {
        WebPMemoryWrite(data, data_size, picture)
    }
    picture.writer = Some(on_write);
    unsafe {
        picture.custom_ptr = writer as *mut c_void;
    };
    // DONE
    (picture, writer)
}

pub fn encode(source: &DynamicImage) -> Vec<u8> {
    let config = init_config();
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
