//! H.264 硬解。macOS：VideoToolbox 解成 NV12（420YpCbCr8BiPlanarFullRange）的 CVPixelBuffer，IOSurface 做底、
//! Metal 能直接用：界面（GPUI 的 `surface()`）拿它当两张纹理画，不经 CPU 拷贝。NV12 也是硬解原生的输出，不用再转色彩格式。
//! 其他平台先没有解码器（界面上提示）。

#[cfg(target_os = "macos")]
pub use mac::{Decoder, Frame};

#[cfg(not(target_os = "macos"))]
pub struct Frame;

#[cfg(not(target_os = "macos"))]
impl Frame {
    pub fn size(&self) -> (u32, u32) {
        (0, 0)
    }
}

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
    pub fn decode(&mut self, _f: &lolidesk_proto::VideoFrame) -> anyhow::Result<Option<Frame>> {
        anyhow::bail!("这个平台还没有视频解码器")
    }
}

#[cfg(not(target_os = "macos"))]
impl Default for Decoder {
    fn default() -> Self {
        Self::new()
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
        kCFAllocatorNull, CFBoolean, CFDictionary, CFNumber, CFRetained, CFString, CFType,
    };
    use objc2_core_media::{
        CMBlockBuffer, CMBlockBufferCreateWithMemoryBlock, CMFormatDescription, CMSampleBuffer,
        CMSampleBufferCreateReady, CMTime, CMVideoFormatDescriptionCreateFromH264ParameterSets,
    };
    use objc2_core_video::{
        kCVPixelBufferIOSurfacePropertiesKey, kCVPixelBufferMetalCompatibilityKey,
        kCVPixelBufferPixelFormatTypeKey, kCVPixelFormatType_420YpCbCr8BiPlanarFullRange,
        CVImageBuffer, CVPixelBuffer, CVPixelBufferGetHeight, CVPixelBufferGetWidth,
    };
    use objc2_video_toolbox::{
        VTDecodeFrameFlags, VTDecodeInfoFlags, VTDecompressionOutputCallbackRecord,
        VTDecompressionSession,
    };

    type OSStatus = i32;

    /// 一帧解好的画面：一份 CVPixelBuffer 的引用。界面拿着它，缓冲池就不会把它发给下一帧重用。
    #[derive(Clone)]
    pub struct Frame(pub CFRetained<CVPixelBuffer>);

    // CVPixelBuffer 是线程安全的 CoreFoundation 对象（引用计数原子），objc2 没标 Send/Sync。
    unsafe impl Send for Frame {}
    unsafe impl Sync for Frame {}

    impl Frame {
        pub fn size(&self) -> (u32, u32) {
            (
                CVPixelBufferGetWidth(&self.0) as u32,
                CVPixelBufferGetHeight(&self.0) as u32,
            )
        }
    }

    struct Shared(Mutex<Option<Frame>>);

    pub struct Decoder {
        session: Option<CFRetained<VTDecompressionSession>>,
        desc: Option<CFRetained<CMFormatDescription>>,
        params: Option<(Vec<u8>, Vec<u8>)>,
        out: Arc<Shared>,
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
        let shared = &*(refcon as *const Shared);
        let pb = NonNull::new_unchecked(image as *mut CVPixelBuffer);
        *shared.0.lock().unwrap() = Some(Frame(CFRetained::retain(pb)));
    }

    impl Default for Decoder {
        fn default() -> Self {
            Self::new()
        }
    }

    impl Decoder {
        pub fn new() -> Self {
            Self {
                session: None,
                desc: None,
                params: None,
                out: Arc::new(Shared(Mutex::new(None))),
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
            // NV12（全范围）、IOSurface 做底、Metal 能直接用：GPUI 的 surface() 只收这种
            let fmt = CFNumber::new_i32(kCVPixelFormatType_420YpCbCr8BiPlanarFullRange as i32);
            let iosurface = CFDictionary::<CFString, CFType>::empty();
            let yes: &CFType = CFBoolean::new(true);
            let attrs = unsafe {
                CFDictionary::<CFString, CFType>::from_slices(
                    &[
                        kCVPixelBufferPixelFormatTypeKey,
                        kCVPixelBufferIOSurfacePropertiesKey,
                        kCVPixelBufferMetalCompatibilityKey,
                    ],
                    &[&fmt, &iosurface, yes],
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

        /// 解一帧。还没等到关键帧、或者这一帧没出画面时返回 None。
        pub fn decode(&mut self, f: &VideoFrame) -> Result<Option<Frame>> {
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
                return Ok(None);
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

    // VideoToolbox 会话和像素缓冲都可以跨线程用；解码跑在自己的线程上，需要 Send。
    unsafe impl Send for Decoder {}
    unsafe impl Send for Shared {}
    unsafe impl Sync for Shared {}

    impl Drop for Decoder {
        fn drop(&mut self) {
            if let Some(s) = self.session.take() {
                unsafe { s.invalidate() };
            }
        }
    }
}

/// VideoToolbox 编一帧、再用 [`Decoder`] 解：出来的必须是同尺寸、NV12 全范围、IOSurface 做底的像素缓冲（GPUI 直接画）。
#[cfg(all(test, target_os = "macos"))]
mod tests {
    #![allow(deprecated, unused_unsafe, non_upper_case_globals)]

    use std::{
        ffi::c_void,
        ptr::{self, NonNull},
        sync::Mutex,
    };

    use bytes::Bytes;
    use lolidesk_proto::VideoFrame;
    use objc2_core_foundation::CFRetained;
    use objc2_core_media::{
        kCMVideoCodecType_H264, CMBlockBufferCopyDataBytes, CMSampleBuffer, CMTimeMake,
        CMVideoFormatDescriptionGetH264ParameterSetAtIndex,
    };
    use objc2_core_video::{
        kCVPixelFormatType_32BGRA, kCVPixelFormatType_420YpCbCr8BiPlanarFullRange, CVImageBuffer,
        CVPixelBuffer, CVPixelBufferCreate, CVPixelBufferGetIOSurface,
        CVPixelBufferGetPixelFormatType,
    };
    use objc2_video_toolbox::{VTCompressionSession, VTEncodeInfoFlags};

    use super::Decoder;

    static OUT: Mutex<Vec<VideoFrame>> = Mutex::new(Vec::new());

    unsafe extern "C-unwind" fn on_encoded(
        _refcon: *mut c_void,
        _src: *mut c_void,
        status: i32,
        _flags: VTEncodeInfoFlags,
        sb: *mut CMSampleBuffer,
    ) {
        assert_eq!(status, 0);
        let sb = &*sb;
        let bb = sb.data_buffer().unwrap();
        let len = bb.data_length();
        let mut data = vec![0u8; len];
        CMBlockBufferCopyDataBytes(&bb, 0, len, NonNull::new(data.as_mut_ptr().cast()).unwrap());
        let desc = sb.format_description().unwrap();
        let mut sets = Vec::new();
        for i in 0..2 {
            let (mut p, mut n, mut count, mut nal): (*const u8, usize, usize, i32) =
                (ptr::null(), 0, 0, 0);
            CMVideoFormatDescriptionGetH264ParameterSetAtIndex(
                &desc, i, &mut p, &mut n, &mut count, &mut nal,
            );
            sets.push(std::slice::from_raw_parts(p, n).to_vec());
        }
        OUT.lock().unwrap().push(VideoFrame {
            key: true,
            sps: Some(sets[0].clone()),
            pps: Some(sets[1].clone()),
            avcc: Bytes::from(data),
        });
    }

    #[test]
    fn encode_then_decode_to_nv12() {
        let (w, h) = (320usize, 192usize);
        unsafe {
            let mut session: *mut VTCompressionSession = ptr::null_mut();
            let st = VTCompressionSession::create(
                None,
                w as i32,
                h as i32,
                kCMVideoCodecType_H264,
                None,
                None,
                None,
                Some(on_encoded),
                ptr::null_mut(),
                NonNull::from(&mut session),
            );
            assert_eq!(st, 0, "VTCompressionSessionCreate");
            let session = CFRetained::from_raw(NonNull::new(session).unwrap());
            let mut pb: *mut CVPixelBuffer = ptr::null_mut();
            let st = CVPixelBufferCreate(
                None,
                w,
                h,
                kCVPixelFormatType_32BGRA,
                None,
                NonNull::from(&mut pb),
            );
            assert_eq!(st, 0, "CVPixelBufferCreate");
            let pb = CFRetained::from_raw(NonNull::new(pb).unwrap());
            let img: &CVImageBuffer = &*(&*pb as *const CVPixelBuffer as *const CVImageBuffer);
            let st = session.encode_frame(
                img,
                CMTimeMake(0, 30),
                CMTimeMake(1, 30),
                None,
                ptr::null_mut(),
                ptr::null_mut(),
            );
            assert_eq!(st, 0, "encode_frame");
            session.complete_frames(CMTimeMake(1, 30));
            session.invalidate();
        }
        let frame = OUT.lock().unwrap().pop().expect("编码器没出帧");
        let mut dec = Decoder::new();
        let out = dec.decode(&frame).unwrap().expect("解码器没出画面");
        assert_eq!(out.size(), (w as u32, h as u32));
        assert_eq!(
            CVPixelBufferGetPixelFormatType(&out.0),
            kCVPixelFormatType_420YpCbCr8BiPlanarFullRange
        );
        assert!(
            CVPixelBufferGetIOSurface(Some(&out.0)).is_some(),
            "要 IOSurface 做底"
        );
    }
}
