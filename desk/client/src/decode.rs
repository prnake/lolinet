//! H.264 解码成 RGBA。macOS 用 VideoToolbox 硬解；其他平台先没有解码器（界面上提示）。

pub struct Rgba {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[cfg(target_os = "macos")]
pub use mac::Decoder;

#[cfg(not(target_os = "macos"))]
pub struct Decoder;

#[cfg(not(target_os = "macos"))]
impl Decoder {
    pub fn new() -> Self {
        Self
    }
    pub fn supported() -> bool {
        false
    }
    pub fn decode(&mut self, _f: &lolidesk_proto::VideoFrame) -> anyhow::Result<Option<Rgba>> {
        anyhow::bail!("这个平台还没有视频解码器")
    }
}

#[cfg(target_os = "macos")]
mod mac {
    #![allow(deprecated, unused_unsafe, non_upper_case_globals)]

    use std::{
        ffi::c_void,
        ptr::{self, NonNull},
        sync::{Arc, Mutex},
    };

    use anyhow::{bail, Context, Result};
    use lolidesk_proto::VideoFrame;
    use objc2_core_foundation::{
        kCFAllocatorNull, CFDictionary, CFNumber, CFRetained, CFString, CFType,
    };
    use objc2_core_media::{
        CMBlockBuffer, CMBlockBufferCreateWithMemoryBlock, CMFormatDescription, CMSampleBuffer,
        CMSampleBufferCreateReady, CMTime, CMVideoFormatDescriptionCreateFromH264ParameterSets,
    };
    use objc2_core_video::{
        kCVPixelBufferPixelFormatTypeKey, kCVPixelFormatType_32BGRA, CVImageBuffer, CVPixelBuffer,
        CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow, CVPixelBufferGetHeight,
        CVPixelBufferGetWidth, CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags,
        CVPixelBufferUnlockBaseAddress,
    };
    use objc2_video_toolbox::{
        VTDecodeFrameFlags, VTDecodeInfoFlags, VTDecompressionOutputCallbackRecord,
        VTDecompressionSession,
    };

    use super::Rgba;

    type OSStatus = i32;

    struct Out(Mutex<Option<Rgba>>);

    pub struct Decoder {
        session: Option<CFRetained<VTDecompressionSession>>,
        desc: Option<CFRetained<CMFormatDescription>>,
        params: Option<(Vec<u8>, Vec<u8>)>,
        out: Arc<Out>,
    }

    unsafe extern "C-unwind" fn on_decoded(
        refcon: *mut c_void,
        _src: *mut c_void,
        status: OSStatus,
        _flags: VTDecodeInfoFlags,
        image: *mut CVImageBuffer,
        _pts: CMTime,
        _dur: CMTime,
    ) {
        if refcon.is_null() || status != 0 || image.is_null() {
            return;
        }
        let out = &*(refcon as *const Out);
        let pb: &CVPixelBuffer = &*(image as *const CVPixelBuffer);
        if CVPixelBufferLockBaseAddress(pb, CVPixelBufferLockFlags::ReadOnly) != 0 {
            return;
        }
        let w = CVPixelBufferGetWidth(pb);
        let h = CVPixelBufferGetHeight(pb);
        let bpr = CVPixelBufferGetBytesPerRow(pb);
        let base = CVPixelBufferGetBaseAddress(pb) as *const u8;
        if !base.is_null() {
            let mut rgba = vec![0u8; w * h * 4];
            for y in 0..h {
                let row = std::slice::from_raw_parts(base.add(y * bpr), w * 4);
                let dst = &mut rgba[y * w * 4..(y + 1) * w * 4];
                for (d, s) in dst
                    .as_chunks_mut::<4>()
                    .0
                    .iter_mut()
                    .zip(row.as_chunks::<4>().0)
                {
                    *d = [s[2], s[1], s[0], 255];
                }
            }
            *out.0.lock().unwrap() = Some(Rgba {
                width: w as u32,
                height: h as u32,
                rgba,
            });
        }
        CVPixelBufferUnlockBaseAddress(pb, CVPixelBufferLockFlags::ReadOnly);
    }

    impl Decoder {
        pub fn new() -> Self {
            Self {
                session: None,
                desc: None,
                params: None,
                out: Arc::new(Out(Mutex::new(None))),
            }
        }

        pub fn supported() -> bool {
            true
        }

        fn setup(&mut self, sps: &[u8], pps: &[u8]) -> Result<()> {
            let mut ptrs = [
                NonNull::new(sps.as_ptr() as *mut u8).context("空 SPS")?,
                NonNull::new(pps.as_ptr() as *mut u8).context("空 PPS")?,
            ];
            let mut sizes = [sps.len(), pps.len()];
            let mut desc_out: *const CMFormatDescription = ptr::null();
            let st = unsafe {
                CMVideoFormatDescriptionCreateFromH264ParameterSets(
                    None,
                    2,
                    NonNull::from(&mut ptrs[0]),
                    NonNull::from(&mut sizes[0]),
                    4,
                    NonNull::from(&mut desc_out),
                )
            };
            if st != 0 {
                bail!("CMVideoFormatDescriptionCreateFromH264ParameterSets {st}");
            }
            let desc = unsafe {
                CFRetained::from_raw(
                    NonNull::new(desc_out as *mut CMFormatDescription)
                        .context("空 format description")?,
                )
            };
            let fmt = CFNumber::new_i32(kCVPixelFormatType_32BGRA as i32);
            let fmt_t: &CFType = &fmt;
            let attrs = unsafe {
                CFDictionary::<CFString, CFType>::from_slices(
                    &[kCVPixelBufferPixelFormatTypeKey],
                    &[fmt_t],
                )
            };
            let record = VTDecompressionOutputCallbackRecord {
                decompressionOutputCallback: Some(on_decoded),
                decompressionOutputRefCon: Arc::as_ptr(&self.out) as *mut c_void,
            };
            let mut out: *mut VTDecompressionSession = ptr::null_mut();
            let st = unsafe {
                VTDecompressionSession::create(
                    None,
                    &desc,
                    None,
                    Some(attrs.as_opaque()),
                    &record,
                    NonNull::from(&mut out),
                )
            };
            if st != 0 {
                bail!("VTDecompressionSessionCreate {st}");
            }
            if let Some(old) = self.session.take() {
                unsafe { old.invalidate() };
            }
            self.session =
                Some(unsafe { CFRetained::from_raw(NonNull::new(out).context("空 session")?) });
            self.desc = Some(desc);
            self.params = Some((sps.to_vec(), pps.to_vec()));
            Ok(())
        }

        pub fn decode(&mut self, f: &VideoFrame) -> Result<Option<Rgba>> {
            if f.key {
                if let (Some(sps), Some(pps)) = (&f.sps, &f.pps) {
                    if self
                        .params
                        .as_ref()
                        .map(|(s, p)| s != sps || p != pps)
                        .unwrap_or(true)
                    {
                        self.setup(sps, pps)?;
                    }
                }
            }
            let (Some(session), Some(desc)) = (&self.session, &self.desc) else {
                return Ok(None); // 还没等到关键帧
            };
            let mut data = f.avcc.to_vec();
            let len = data.len();
            if len == 0 {
                return Ok(None);
            }
            unsafe {
                let mut bb_out: *mut CMBlockBuffer = ptr::null_mut();
                let st = CMBlockBufferCreateWithMemoryBlock(
                    None,
                    data.as_mut_ptr() as *mut c_void,
                    len,
                    kCFAllocatorNull,
                    ptr::null(),
                    0,
                    len,
                    0,
                    NonNull::from(&mut bb_out),
                );
                if st != 0 {
                    bail!("CMBlockBufferCreateWithMemoryBlock {st}");
                }
                let bb = CFRetained::from_raw(NonNull::new(bb_out).context("空 block buffer")?);
                let mut sb_out: *mut CMSampleBuffer = ptr::null_mut();
                let st = CMSampleBufferCreateReady(
                    None,
                    Some(&bb),
                    Some(desc),
                    1,
                    0,
                    ptr::null(),
                    1,
                    &len,
                    NonNull::from(&mut sb_out),
                );
                if st != 0 {
                    bail!("CMSampleBufferCreateReady {st}");
                }
                let sb = CFRetained::from_raw(NonNull::new(sb_out).context("空 sample buffer")?);
                let st = session.decode_frame(
                    &sb,
                    VTDecodeFrameFlags::empty(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                );
                if st != 0 {
                    bail!("decode_frame {st}");
                }
                session.wait_for_asynchronous_frames();
            }
            drop(data);
            Ok(self.out.0.lock().unwrap().take())
        }
    }

    // VideoToolbox 会话可以跨线程使用；解码循环跑在 tokio 任务里，需要 Send。
    unsafe impl Send for Decoder {}

    impl Drop for Decoder {
        fn drop(&mut self) {
            if let Some(s) = self.session.take() {
                unsafe { s.invalidate() };
            }
        }
    }
}
