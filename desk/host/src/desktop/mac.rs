//! macOS：ScreenCaptureKit 采集 → VideoToolbox 硬件 H.264（低延迟档）→ 每帧一个 Vec<u8>（proto::VideoFrame 编码）。
//! 和 Vibe42 一样：Idle 帧不编码；光标不烧进画面（客户端用本地光标）；丢帧后靠关键帧请求恢复。

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
use lolidesk_proto::{DisplayInfo, Quality, VideoFrame};
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

use super::{bitrate, target_size, CaptureConfig};

type OSStatus = i32;

fn pixel_size(id: u32, points: (u32, u32)) -> (u32, u32) {
    unsafe {
        match CGDisplayCopyDisplayMode(id) {
            Some(m) => (
                CGDisplayMode::pixel_width(Some(&m)) as u32,
                CGDisplayMode::pixel_height(Some(&m)) as u32,
            ),
            None => points,
        }
    }
}

fn ensure_permission() -> Result<()> {
    if unsafe { CGPreflightScreenCaptureAccess() } {
        return Ok(());
    }
    unsafe { CGRequestScreenCaptureAccess() };
    bail!("没有屏幕录制权限：系统设置 → 隐私与安全性 → 屏幕录制，勾上 lolidesk-host（或启动它的终端），然后重启服务")
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
            let (w, h) = pixel_size(id, (d.width(), d.height()));
            DisplayInfo {
                id,
                width: w,
                height: h,
                primary: id == main,
                name: format!("显示器 {id} · {w}×{h}"),
            }
        })
        .collect())
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
    force_key: AtomicBool,
    frames: AtomicU64,
    fps: u32,
    size: (u32, u32),
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

    fn create_session(&self, quality: Quality, low_latency: bool) -> Result<Session> {
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
        let br = bitrate(quality, w, h) as i32;
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
        let (pw, ph) = pixel_size(cfg.display, (display.width(), display.height()));
        let (w, h) = target_size(cfg.quality, pw, ph);
        let filter = SCContentFilter::create()
            .with_display(&display)
            .with_excluding_windows(&[])
            .build()
            .context("SCContentFilter")?;
        let config = SCStreamConfiguration::new()
            .with_width(w)
            .with_height(h)
            .with_pixel_format(PixelFormat::BGRA)
            .with_fps(cfg.fps as u32)
            .with_queue_depth(3)
            .with_shows_cursor(false);
        let inner = Arc::new(Inner {
            tx,
            force_key: AtomicBool::new(true),
            frames: AtomicU64::new(0),
            fps: cfg.fps as u32,
            size: (w, h),
            params: Mutex::new(None),
            session: Mutex::new(None),
        });
        let session = match inner.create_session(cfg.quality, true) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("低延迟编码器不可用（{e:#}），退回普通硬件编码器");
                inner.create_session(cfg.quality, false)?
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

    pub fn size(&self) -> (u32, u32) {
        self.inner.size
    }

    pub fn request_keyframe(&self) {
        self.inner.force_key.store(true, Ordering::Relaxed);
    }

    /// 第一版只换码率（尺寸不变）：重建编码器，下一帧是关键帧。
    pub fn set_quality(&self, q: Quality) {
        match self
            .inner
            .create_session(q, true)
            .or_else(|_| self.inner.create_session(q, false))
        {
            Ok(s) => {
                let old = self.inner.session.lock().unwrap().replace(s);
                if let Some(old) = old {
                    unsafe { old.invalidate() };
                }
                self.inner.force_key.store(true, Ordering::Relaxed);
            }
            Err(e) => tracing::warn!("换画质失败：{e:#}"),
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
