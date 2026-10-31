#[cfg(target_os = "macos")]
pub fn ensure_permission() -> Result<(), String> {
    use block2::RcBlock;
    use objc2::runtime::Bool;
    use objc2_av_foundation::{AVAuthorizationStatus, AVCaptureDevice, AVMediaTypeAudio};
    use std::sync::mpsc;
    use std::time::Duration;

    // AVMediaTypeAudio is an immutable framework constant and a valid media type
    // for both AVFoundation authorization calls below.
    let media_type = unsafe { AVMediaTypeAudio }.ok_or("macOS 未提供音訊授權介面")?;
    let status = unsafe { AVCaptureDevice::authorizationStatusForMediaType(media_type) };
    let granted = match status {
        AVAuthorizationStatus::Authorized => true,
        AVAuthorizationStatus::NotDetermined => {
            let (sender, receiver) = mpsc::channel();
            let completion = RcBlock::new(move |granted: Bool| {
                let _ = sender.send(granted.as_bool());
            });
            // AVFoundation copies the block and invokes it on its own queue.
            // Only a boolean is passed back to the recording command.
            unsafe {
                AVCaptureDevice::requestAccessForMediaType_completionHandler(
                    media_type,
                    &completion,
                );
            }
            receiver
                .recv_timeout(Duration::from_secs(120))
                .map_err(|_| "請先完成 macOS 麥克風授權，再按住錄音鍵重試")?
        }
        _ => false,
    };
    if granted {
        Ok(())
    } else {
        Err("未獲准使用麥克風：請到系統設定 → 私隱與保安 → 麥克風，啟用 GPT-Typeless，再重新開啟程式".into())
    }
}

#[cfg(not(target_os = "macos"))]
pub fn ensure_permission() -> Result<(), String> {
    Ok(())
}
