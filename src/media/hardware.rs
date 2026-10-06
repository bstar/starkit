//! Optional FFmpeg device decoding. Downloaded frames retain timestamps/color.
use super::*;
use av::{codec, frame};

unsafe extern "C" fn choose_format(
    context: *mut av::ffi::AVCodecContext,
    formats: *const av::ffi::AVPixelFormat,
) -> av::ffi::AVPixelFormat {
    let requested = unsafe { (*context).opaque as isize as i32 };
    let mut cursor = formats;
    while unsafe { *cursor } != av::ffi::AVPixelFormat::AV_PIX_FMT_NONE {
        if unsafe { *cursor as i32 } == requested {
            return unsafe { *cursor };
        }
        cursor = unsafe { cursor.add(1) };
    }
    unsafe { av::ffi::avcodec_default_get_format(context, formats) }
}
pub fn decoder(parameters: codec::Parameters) -> Result<codec::decoder::Video> {
    if std::env::var_os("STAR_VIDEO_SOFTWARE").is_none() {
        if let Ok(decoder) = accelerated(parameters.clone()) {
            return Ok(decoder);
        }
    }
    let mut context = codec::context::Context::from_parameters(parameters)?;
    context.set_threading(codec::threading::Config::count(
        std::thread::available_parallelism().map_or(2, |n| n.get().min(8)),
    ));
    tracing::info!("Video decoding: software");
    Ok(context.decoder().video()?)
}
fn accelerated(parameters: codec::Parameters) -> Result<codec::decoder::Video> {
    let kind = if cfg!(target_os = "macos") {
        av::ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_VIDEOTOOLBOX
    } else {
        av::ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_VAAPI
    };
    let codec = codec::decoder::find(parameters.id()).context("Video decoder unavailable")?;
    let mut format = None;
    for index in 0..32 {
        let config = unsafe { av::ffi::avcodec_get_hw_config(codec.as_ptr(), index) };
        if config.is_null() {
            break;
        }
        let config = unsafe { &*config };
        if config.device_type == kind
            && config.methods & av::ffi::AV_CODEC_HW_CONFIG_METHOD_HW_DEVICE_CTX as i32 != 0
        {
            format = Some(config.pix_fmt);
            break;
        }
    }
    let format = format.context("Hardware decoder unsupported")?;
    let mut context = codec::context::Context::from_parameters(parameters)?;
    let mut device = std::ptr::null_mut();
    let result = unsafe {
        av::ffi::av_hwdevice_ctx_create(
            &mut device,
            kind,
            std::ptr::null(),
            std::ptr::null_mut(),
            0,
        )
    };
    anyhow::ensure!(result >= 0, "Hardware video device unavailable");
    unsafe {
        (*context.as_mut_ptr()).hw_device_ctx = device;
        (*context.as_mut_ptr()).get_format = Some(choose_format);
        (*context.as_mut_ptr()).opaque = format as isize as *mut std::ffi::c_void;
    }
    let decoder = context.decoder().video()?;
    tracing::info!(?kind, "Video hardware decoder initialized");
    Ok(decoder)
}
pub fn download(frame: &frame::Video) -> Result<frame::Video> {
    if unsafe { (*frame.as_ptr()).hw_frames_ctx.is_null() } {
        let mut output = frame::Video::empty();
        let result = unsafe { av::ffi::av_frame_ref(output.as_mut_ptr(), frame.as_ptr()) };
        anyhow::ensure!(result >= 0, "Video frame reference failed");
        return Ok(output);
    }
    let mut output = frame::Video::empty();
    let result =
        unsafe { av::ffi::av_hwframe_transfer_data(output.as_mut_ptr(), frame.as_ptr(), 0) };
    anyhow::ensure!(
        result >= 0,
        "Hardware video frame transfer failed: {result}"
    );
    let result = unsafe { av::ffi::av_frame_copy_props(output.as_mut_ptr(), frame.as_ptr()) };
    anyhow::ensure!(result >= 0, "Hardware video metadata copy failed");
    Ok(output)
}
