//! macOS：ScreenCaptureKit 采集 → VideoToolbox 硬件 H.264（低延迟档）→ 每帧一个 Vec<u8>（proto::VideoFrame 编码）。
//! Idle 帧不编码；光标不烧进画面（客户端用本地光标）；丢帧后靠关键帧请求恢复。

#![allow(
    deprecated,
    unused_unsafe,
    non_upper_case_globals,
    clippy::missing_safety_doc
)]

use std::{
    ffi::c_void,
    ptr::{self, NonNull},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
};

use anyhow::{bail, Context, Result};
use bytes::Bytes;
use lolidesk_proto::{bitrate, target_size, DisplayInfo, VideoFrame};
use objc2_core_foundation::{CFBoolean, CFDictionary, CFNumber, CFRetained, CFString, CFType};
use objc2_core_graphics::{
    CGDisplayCopyDisplayMode, CGDisplayMode, CGMainDisplayID, CGPreflightScreenCaptureAccess,
    CGRequestScreenCaptureAccess,
};
use objc2_core_media::{
    kCMVideoCodecType_H264, CMBlockBufferCopyDataBytes, CMSampleBuffer, CMTimeMake,
    CMVideoFormatDescriptionGetH264ParameterSetAtIndex,
};
use objc2_core_video::CVImageBuffer;
use objc2_video_toolbox::{
    kVTCompressionPropertyKey_AllowFrameReordering, kVTCompressionPropertyKey_AverageBitRate,
    kVTCompressionPropertyKey_ExpectedFrameRate, kVTCompressionPropertyKey_MaxKeyFrameInterval,
    kVTCompressionPropertyKey_MaxKeyFrameIntervalDuration,
    kVTCompressionPropertyKey_PrioritizeEncodingSpeedOverQuality,
    kVTCompressionPropertyKey_ProfileLevel, kVTCompressionPropertyKey_RealTime,
    kVTEncodeFrameOptionKey_ForceKeyFrame, kVTProfileLevel_H264_High_AutoLevel,
    kVTVideoEncoderSpecification_EnableHardwareAcceleratedVideoEncoder,
    kVTVideoEncoderSpecification_EnableLowLatencyRateControl, VTCompressionSession,
    VTEncodeInfoFlags, VTSessionSetProperty,
};
use screencapturekit::prelude::*;
use screencapturekit::stream::configuration::PixelFormat;
use tokio::sync::mpsc::Sender;

use super::{clamp_fps, Actual, CaptureConfig};

type OSStatus = i32;

/// 显示器的原生像素和刷新率（Hz，不知道是 0）。
fn display_mode(id: u32, points: (u32, u32)) -> ((u32, u32), u16) {
    unsafe {
        match CGDisplayCopyDisplayMode(id) {
            Some(m) => (
                (
                    CGDisplayMode::pixel_width(Some(&m)) as u32,
                    CGDisplayMode::pixel_height(Some(&m)) as u32,
                ),
                CGDisplayMode::refresh_rate(Some(&m)).round() as u16,
            ),
            None => (points, 0),
        }
    }
}

fn ensure_permission() -> Result<()> {
    if unsafe { CGPreflightScreenCaptureAccess() } {
        return Ok(());
    }
    unsafe { CGRequestScreenCaptureAccess() };
    bail!("没有屏幕录制权限：系统设置 → 隐私与安全性 → 屏幕录制，勾上 lolidesk（或启动它的终端），然后重启服务（lolidesk host install）")
}

pub fn list_displays() -> Result<Vec<DisplayInfo>> {
    ensure_permission()?;
    let content = SCShareableContent::get().context("SCShareableContent")?;
    let main = unsafe { CGMainDisplayID() };
    Ok(content
        .displays()
        .into_iter()
        .map(|d| {
            let id = d.display_id();
            let ((w, h), refresh) = display_mode(id, (d.width(), d.height()));
            // 内建屏的刷新率是 0（ProMotion 可变刷新），按 120 报
            let refresh = if refresh == 0 && builtin(id) {
                120
            } else {
                refresh
            };
            DisplayInfo {
                id,
                width: w,
                height: h,
                refresh,
                primary: id == main,
                name: if builtin(id) {
                    "内建显示器".into()
                } else {
                    format!("显示器 {id}")
                },
            }
        })
        .collect())
}

/// 截一张图给 agent：SCScreenshotManager 按要的尺寸出 CGImage（带光标，agent 要知道鼠标在哪），
/// ImageIO 编 JPEG。ImageIO 只能写文件，所以写到一个 700 的临时目录里再读回来。
pub fn screenshot(display: u32, max_edge: u32, quality: u8) -> Result<super::Shot> {
    use screencapturekit::screenshot_manager::{CGImageExt, ImageFormat, SCScreenshotManager};
    ensure_permission()?;
    let content = SCShareableContent::get().context("SCShareableContent")?;
    let d = content
        .displays()
        .into_iter()
        .find(|d| d.display_id() == display)
        .with_context(|| format!("没有显示器 {display}"))?;
    let ((pw, ph), _) = display_mode(display, (d.width(), d.height()));
    let (w, h) = target_size(max_edge, pw, ph);
    let filter = SCContentFilter::create()
        .with_display(&d)
        .with_excluding_windows(&[])
        .build()
        .context("SCContentFilter")?;
    let config = SCStreamConfiguration::new()
        .with_width(w)
        .with_height(h)
        .with_shows_cursor(true);
    let img = SCScreenshotManager::capture_image(&filter, &config).context("截图")?;
    let dir = std::env::temp_dir().join(format!("lolidesk-shot-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    }
    let path = dir.join(format!("{}.jpg", rand::random::<u64>()));
    let q = quality.clamp(1, 100) as f32 / 100.0;
    let res = img
        .save(&path.to_string_lossy(), ImageFormat::Jpeg(q))
        .map_err(|e| anyhow::anyhow!("编 JPEG：{e}"))
        .and_then(|()| std::fs::read(&path).context("读截图"));
    let _ = std::fs::remove_file(&path);
    Ok(super::Shot {
        jpeg: res?,
        width: w,
        height: h,
        native_width: pw,
        native_height: ph,
    })
}

fn builtin(id: u32) -> bool {
    unsafe { objc2_core_graphics::CGDisplayIsBuiltin(id) }
}

/// VTCompressionSession 是线程安全的 CoreFoundation 对象，objc2 的绑定没标 Send/Sync，这里包一层。
struct Session(CFRetained<VTCompressionSession>);
unsafe impl Send for Session {}
unsafe impl Sync for Session {}

impl std::ops::Deref for Session {
    type Target = VTCompressionSession;
    fn deref(&self) -> &VTCompressionSession {
        &self.0
    }
}

struct Inner {
    tx: Sender<Vec<u8>>,
    force_key: Arc<AtomicBool>,
    frames: AtomicU64,
    fps: u32,
    size: (u32, u32),
    bitrate: u32,
    params: Mutex<Option<(Vec<u8>, Vec<u8>)>>,
    session: Mutex<Option<Session>>,
}

impl Inner {
    fn set_prop(session: &VTCompressionSession, key: &CFString, value: &CFType) {
        let st = unsafe { VTSessionSetProperty(session, key, Some(value)) };
        if st != 0 {
            tracing::debug!(?key, st, "VTSessionSetProperty 不支持");
        }
    }

    fn create_session(&self, low_latency: bool) -> Result<Session> {
        let (w, h) = self.size;
        let yes: &CFType = CFBoolean::new(true);
        let no: &CFType = CFBoolean::new(false);
        let spec = unsafe {
            if low_latency {
                CFDictionary::<CFString, CFType>::from_slices(
                    &[
                        kVTVideoEncoderSpecification_EnableHardwareAcceleratedVideoEncoder,
                        kVTVideoEncoderSpecification_EnableLowLatencyRateControl,
                    ],
                    &[yes, yes],
                )
            } else {
                CFDictionary::<CFString, CFType>::from_slices(
                    &[kVTVideoEncoderSpecification_EnableHardwareAcceleratedVideoEncoder],
                    &[yes],
                )
            }
        };
        let mut out: *mut VTCompressionSession = ptr::null_mut();
        let st = unsafe {
            VTCompressionSession::create(
                None,
                w as i32,
                h as i32,
                kCMVideoCodecType_H264,
                Some(spec.as_opaque()),
                None,
                None,
                Some(on_encoded),
                self as *const Inner as *mut c_void,
                NonNull::from(&mut out),
            )
        };
        if st != 0 {
            bail!("VTCompressionSessionCreate 失败：{st}（low_latency={low_latency}）");
        }
        let session =
            Session(unsafe { CFRetained::from_raw(NonNull::new(out).context("空 session")?) });
        let br = self.bitrate as i32;
        unsafe {
            Self::set_prop(&session, kVTCompressionPropertyKey_RealTime, yes);
            Self::set_prop(
                &session,
                kVTCompressionPropertyKey_ProfileLevel,
                kVTProfileLevel_H264_High_AutoLevel,
            );
            Self::set_prop(&session, kVTCompressionPropertyKey_AllowFrameReordering, no);
            Self::set_prop(
                &session,
                kVTCompressionPropertyKey_PrioritizeEncodingSpeedOverQuality,
                yes,
            );
            Self::set_prop(
                &session,
                kVTCompressionPropertyKey_AverageBitRate,
                &CFNumber::new_i32(br),
            );
            Self::set_prop(
                &session,
                kVTCompressionPropertyKey_ExpectedFrameRate,
                &CFNumber::new_i32(self.fps as i32),
            );
            Self::set_prop(
                &session,
                kVTCompressionPropertyKey_MaxKeyFrameInterval,
                &CFNumber::new_i32(self.fps as i32 * 4),
            );
            Self::set_prop(
                &session,
                kVTCompressionPropertyKey_MaxKeyFrameIntervalDuration,
                &CFNumber::new_i32(4),
            );
            let st = session.prepare_to_encode_frames();
            if st != 0 {
                bail!("prepare_to_encode_frames 失败：{st}");
            }
        }
        tracing::info!(
            w,
            h,
            bitrate = br,
            fps = self.fps,
            low_latency,
            "H.264 编码器就绪"
        );
        Ok(session)
    }

    fn encode(&self, pb: &screencapturekit::CVPixelBuffer) {
        let guard = self.session.lock().unwrap();
        let Some(session) = guard.as_ref() else {
            return;
        };
        let force = self.force_key.swap(false, Ordering::Relaxed);
        let n = self.frames.fetch_add(1, Ordering::Relaxed);
        let pts = unsafe { CMTimeMake(n as i64, self.fps as i32) };
        let dur = unsafe { CMTimeMake(1, self.fps as i32) };
        let yes: &CFType = CFBoolean::new(true);
        let props = force.then(|| unsafe {
            CFDictionary::<CFString, CFType>::from_slices(
                &[kVTEncodeFrameOptionKey_ForceKeyFrame],
                &[yes],
            )
        });
        let img: &CVImageBuffer = unsafe { &*(pb.as_ptr() as *const CVImageBuffer) };
        let st = unsafe {
            session.encode_frame(
                img,
                pts,
                dur,
                props.as_ref().map(|d| d.as_opaque()),
                ptr::null_mut(),
                ptr::null_mut::<VTEncodeInfoFlags>(),
            )
        };
        if st != 0 {
            tracing::warn!(st, "encode_frame 失败");
        }
    }

    /// 编码器回调：取 AVCC 数据、关键帧时顺带取 SPS/PPS，打包发出去。
    unsafe fn deliver(&self, sb: &CMSampleBuffer) -> Result<()> {
        let bb = sb.data_buffer().context("没有 data buffer")?;
        let len = bb.data_length();
        let mut data = vec![0u8; len];
        let st = CMBlockBufferCopyDataBytes(
            &bb,
            0,
            len,
            NonNull::new(data.as_mut_ptr() as *mut c_void).unwrap(),
        );
        if st != 0 {
            bail!("CMBlockBufferCopyDataBytes {st}");
        }
        let key = avcc_has_idr(&data);
        let (mut sps, mut pps) = (None, None);
        if key {
            if let Some(desc) = sb.format_description() {
                let mut got = Vec::new();
                for i in 0..2usize {
                    let mut p: *const u8 = ptr::null();
                    let mut n: usize = 0;
                    let mut count: usize = 0;
                    let mut nal: std::ffi::c_int = 0;
                    let st = CMVideoFormatDescriptionGetH264ParameterSetAtIndex(
                        &desc, i, &mut p, &mut n, &mut count, &mut nal,
                    );
                    if st == 0 && !p.is_null() {
                        got.push(std::slice::from_raw_parts(p, n).to_vec());
                    }
                }
                if got.len() == 2 {
                    *self.params.lock().unwrap() = Some((got[0].clone(), got[1].clone()));
                }
            }
            if let Some((s, p)) = self.params.lock().unwrap().clone() {
                sps = Some(s);
                pps = Some(p);
            }
        }
        let frame = VideoFrame {
            key,
            sps,
            pps,
            avcc: Bytes::from(data),
        }
        .encode();
        if let Err(tokio::sync::mpsc::error::TrySendError::Full(_)) = self.tx.try_send(frame) {
            // 发送队列满就丢这一帧，下一帧强制关键帧，解码端不会卡在参考缺失上
            self.force_key.store(true, Ordering::Relaxed);
        }
        Ok(())
    }
}

/// AVCC：4 字节大端长度 + NALU。NAL type 5 = IDR。
fn avcc_has_idr(data: &[u8]) -> bool {
    let mut i = 0usize;
    while i + 5 <= data.len() {
        let n = u32::from_be_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]) as usize;
        let t = data[i + 4] & 0x1f;
        if t == 5 {
            return true;
        }
        i += 4 + n;
    }
    false
}

unsafe extern "C-unwind" fn on_encoded(
    refcon: *mut c_void,
    _src: *mut c_void,
    status: OSStatus,
    _flags: VTEncodeInfoFlags,
    sb: *mut CMSampleBuffer,
) {
    if refcon.is_null() || status != 0 || sb.is_null() {
        return;
    }
    let inner = &*(refcon as *const Inner);
    if let Err(e) = inner.deliver(&*sb) {
        tracing::debug!("编码输出处理失败：{e:#}");
    }
}

struct Handler(Arc<Inner>);

impl SCStreamOutputTrait for Handler {
    fn did_output_sample_buffer(&self, sample: CMSampleBufferSC, of_type: SCStreamOutputType) {
        if !matches!(of_type, SCStreamOutputType::Screen) {
            return;
        }
        // Idle / Blank 帧：屏幕没变，不编码；但客户端刚要了关键帧时照编，否则它一直收不到
        let has_content = sample
            .frame_status()
            .map(|s| s.has_content())
            .unwrap_or(true);
        if !has_content && !self.0.force_key.load(Ordering::Relaxed) {
            return;
        }
        if let Some(pb) = sample.pixel_buffer() {
            self.0.encode(&pb);
        }
    }
}

type CMSampleBufferSC = screencapturekit::cm::CMSampleBuffer;

struct SendStream(SCStream);
// SCStream 是 ObjC 对象，start/stop 可以在任意线程调；这里只是让它能放进 tokio 任务里。
unsafe impl Send for SendStream {}

pub struct Capture {
    stream: SendStream,
    inner: Arc<Inner>,
}

impl Capture {
    pub fn start(cfg: CaptureConfig, tx: Sender<Vec<u8>>) -> Result<Self> {
        ensure_permission()?;
        let content = SCShareableContent::get().context("SCShareableContent")?;
        let display = content
            .displays()
            .into_iter()
            .find(|d| d.display_id() == cfg.display)
            .with_context(|| format!("没有显示器 {}", cfg.display))?;
        let ((pw, ph), refresh) = display_mode(cfg.display, (display.width(), display.height()));
        let refresh = if refresh == 0 && builtin(cfg.display) {
            120
        } else {
            refresh
        };
        let (w, h) = target_size(cfg.video.long_edge, pw, ph);
        let fps = clamp_fps(cfg.video.fps, refresh);
        let br = bitrate(cfg.video.quality, w, h, fps);
        let filter = SCContentFilter::create()
            .with_display(&display)
            .with_excluding_windows(&[])
            .build()
            .context("SCContentFilter")?;
        let config = SCStreamConfiguration::new()
            .with_width(w)
            .with_height(h)
            .with_pixel_format(PixelFormat::BGRA)
            .with_fps(fps as u32)
            .with_queue_depth(3)
            .with_shows_cursor(false);
        cfg.force_key.store(true, Ordering::Relaxed);
        let inner = Arc::new(Inner {
            tx,
            force_key: cfg.force_key.clone(),
            frames: AtomicU64::new(0),
            fps: fps as u32,
            size: (w, h),
            bitrate: br,
            params: Mutex::new(None),
            session: Mutex::new(None),
        });
        let session = match inner.create_session(true) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("低延迟编码器不可用（{e:#}），退回普通硬件编码器");
                inner.create_session(false)?
            }
        };
        *inner.session.lock().unwrap() = Some(session);
        let mut stream = SCStream::new(&filter, &config).context("SCStream")?;
        stream
            .add_output_handler(Handler(inner.clone()), SCStreamOutputType::Screen)
            .context("add_output_handler")?;
        stream.start_capture().context("start_capture")?;
        Ok(Self {
            stream: SendStream(stream),
            inner,
        })
    }

    pub fn actual(&self) -> Actual {
        Actual {
            width: self.inner.size.0,
            height: self.inner.size.1,
            fps: self.inner.fps as u8,
            bitrate: self.inner.bitrate,
        }
    }

    pub fn stop(self) {
        let _ = self.stream.0.stop_capture();
        if let Some(s) = self.inner.session.lock().unwrap().take() {
            unsafe {
                let _ = s.complete_frames(CMTimeMake(0, 1));
                s.invalidate();
            }
        }
    }
}
