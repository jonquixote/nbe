//! AAC-LC encode for recording (Prompt 09 WU34, SPEC §9.3/§9.4).
//!
//! The engine's master mix is f32 PCM 48 kHz stereo (`audio.rs`); the file
//! needs real AAC frames, not PCM-in-MP4. This module is the ONLY AudioToolbox
//! call site for that conversion, and it lives here — in the one crate the
//! workspace allows `unsafe` in — for the same reason `encode.rs` does: every
//! AudioToolbox call is `unsafe` by construction, and `nbe-engine` denies
//! `unsafe_code`. The engine's `record/aac.rs` is the safe integration layer
//! over this API; no `unsafe` exists on that side.
//!
//! Design notes:
//! - Input is exactly the engine's mix format: interleaved float32, 48 kHz,
//!   stereo. Anything else is refused loudly (`AacError`), never adapted —
//!   the engine produces one format and silent adaptation is how a wrong-rate
//!   recording happens.
//! - Output is AAC-LC (`kAudioFormatMPEG4AAC`) at 192 kbps (SPEC §9.4's audio
//!   bitrate), one `AacFrame` per 1024-sample packet, raw frames (no ADTS —
//!   the MP4 `esds` box carries the `AudioSpecificConfig` instead).
//! - `audio_specific_config` is the converter's compression magic cookie,
//!   read back after creation. The writer needs it for `moov` BEFORE any
//!   sample exists, which is why it is exposed separately rather than
//!   returned with the first frames.
//! - Priming is NOT trimmed here: the first ~2112 decoded samples are
//!   encoder delay (standard AAC behavior). The recording writer trims it at
//!   mux time (drops the first 2 packets, 2048 samples; 64-sample / 1.33 ms
//!   residual inside the 20 ms sync requirement — see
//!   `nbe-engine/src/record/writer.rs`). Container durations are computed
//!   from retained packet counts so `ffprobe` duration stays exact.
//!
//! Failure shape: there is no fallback. AudioToolbox absent (or refusing the
//! format) yields `AacError::Unavailable` — the writer fails loudly rather
//! than substituting PCM.

use std::ffi::c_void;
use std::ptr::NonNull;

use objc2_audio_toolbox::{
    kAudioConverterCompressionMagicCookie, kAudioConverterEncodeBitRate, AudioConverterDispose,
    AudioConverterFillComplexBuffer, AudioConverterGetProperty, AudioConverterGetPropertyInfo,
    AudioConverterNew, AudioConverterRef, AudioConverterSetProperty,
};
use objc2_core_audio_types::{
    kAudioFormatFlagIsFloat, kAudioFormatFlagIsPacked, kAudioFormatLinearPCM, kAudioFormatMPEG4AAC,
    AudioBuffer, AudioBufferList, AudioStreamBasicDescription, AudioStreamPacketDescription,
};
use thiserror::Error;

/// One AAC-LC packet: 1024 sample frames of raw (no ADTS) payload.
#[derive(Debug, Clone)]
pub struct AacFrame {
    pub data: Vec<u8>,
}

/// AAC encode failure. Exactly one class by design: anything that stops the
/// encode — no AudioToolbox, a refused format, a mid-stream OSStatus — means
/// AAC is unavailable, and there is no fallback to degrade to.
#[derive(Debug, Error)]
pub enum AacError {
    /// AudioToolbox AAC encode is unavailable, or the encode failed.
    #[error("E_AAC_UNAVAILABLE: {0}")]
    Unavailable(String),
}

impl AacError {
    /// Stable token for operator-facing state.
    pub fn kind_token(&self) -> &'static str {
        match self {
            AacError::Unavailable(_) => "E_AAC_UNAVAILABLE",
        }
    }
}

fn unavailable(detail: impl Into<String>) -> AacError {
    AacError::Unavailable(detail.into())
}

/// The engine's mix format, the only input this encoder accepts.
pub const INPUT_SAMPLE_RATE: u32 = 48_000;
pub const INPUT_CHANNELS: u32 = 2;
/// AAC-LC packet duration in sample frames.
pub const FRAMES_PER_PACKET: usize = 1024;
/// SPEC §9.4's audio bitrate.
const ENCODE_BIT_RATE: u32 = 192_000;

fn input_asbd() -> AudioStreamBasicDescription {
    AudioStreamBasicDescription {
        mSampleRate: INPUT_SAMPLE_RATE as f64,
        mFormatID: kAudioFormatLinearPCM,
        mFormatFlags: kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked,
        mBytesPerPacket: 8,
        mFramesPerPacket: 1,
        mBytesPerFrame: 8,
        mChannelsPerFrame: INPUT_CHANNELS,
        mBitsPerChannel: 32,
        mReserved: 0,
    }
}

fn output_asbd() -> AudioStreamBasicDescription {
    AudioStreamBasicDescription {
        mSampleRate: INPUT_SAMPLE_RATE as f64,
        mFormatID: kAudioFormatMPEG4AAC,
        mFormatFlags: 0,
        mBytesPerPacket: 0,
        mFramesPerPacket: FRAMES_PER_PACKET as u32,
        mBytesPerFrame: 0,
        mChannelsPerFrame: INPUT_CHANNELS,
        mBitsPerChannel: 0,
        mReserved: 0,
    }
}

/// State borrowed by the input proc during one `FillComplexBuffer` call.
///
/// The proc runs synchronously on the calling thread, so this never crosses
/// threads: it is a stack borrow, not shared state.
struct FeedState {
    /// Next unread input frame (stereo pair).
    cursor: *const f32,
    frames_left: usize,
    /// True only while flushing: input exhaustion is end-of-stream.
    eos: bool,
}

/// Sentinel OSStatus meaning "input temporarily exhausted, feed more later".
/// Distinct from end-of-stream (0 packets + `noErr`, used only at flush):
/// per the AudioConverter docs, a proc that returns an error with zero
/// packets makes `FillComplexBuffer` stop and hand back what it produced so
/// far ALONG WITH the error — which is exactly the "out of input for now"
/// signal. Real converter errors are four-char codes; small 1 cannot be one.
const INPUT_DRAINED: i32 = 1;

/// The `AudioConverterComplexInputDataProc`: hand the converter up to what it
/// asks for from the remaining input, advance the cursor. Zero frames left
/// means end-of-data ONLY at flush (`eos`); mid-stream it returns
/// [`INPUT_DRAINED`] so the converter holds its state instead of finalizing
/// the stream. (Returning 0 packets + `noErr` mid-stream would END the
/// stream — the first chunk would encode and every later chunk would produce
/// nothing, a failure this exact shape once caused.)
unsafe extern "C-unwind" fn input_proc(
    _converter: AudioConverterRef,
    io_packets: NonNull<u32>,
    io_data: NonNull<AudioBufferList>,
    _descs: *mut *mut AudioStreamPacketDescription,
    user_data: *mut c_void,
) -> i32 {
    unsafe {
        let want = *io_packets.as_ptr() as usize;
        let state = &mut *(user_data as *mut FeedState);
        if state.frames_left == 0 {
            *io_packets.as_ptr() = 0;
            return if state.eos { 0 } else { INPUT_DRAINED };
        }
        let give = want.min(state.frames_left);
        let list = io_data.as_ptr();
        (*list).mNumberBuffers = 1;
        let buf = &mut (*list).mBuffers[0] as *mut AudioBuffer;
        (*buf).mNumberChannels = INPUT_CHANNELS;
        (*buf).mDataByteSize = (give * INPUT_CHANNELS as usize * 4) as u32;
        (*buf).mData = state.cursor as *mut c_void;
        state.cursor = state.cursor.add(give * INPUT_CHANNELS as usize);
        state.frames_left -= give;
        *io_packets.as_ptr() = give as u32;
        0
    }
}

/// Streaming AAC-LC encoder over one recording's audio.
///
/// Feed mix PCM with [`Self::encode_interleaved_f32`]; everything fed is
/// consumed (the converter may buffer internally — that is its business, not
/// retained here). [`Self::flush`] signals end-of-data and collects the tail.
pub struct AacEncoder {
    converter: AudioConverterRef,
    cookie: Vec<u8>,
    fed_frames: u64,
}

impl AacEncoder {
    /// Open the PCM→AAC converter and read back the magic cookie.
    pub fn new() -> Result<Self, AacError> {
        let mut input = input_asbd();
        let mut output = output_asbd();
        let mut converter: AudioConverterRef = std::ptr::null_mut();
        let status = unsafe {
            AudioConverterNew(
                NonNull::from(&mut input),
                NonNull::from(&mut output),
                NonNull::new(&mut converter as *mut AudioConverterRef)
                    .expect("a stack out-pointer is non-null"),
            )
        };
        if status != 0 || converter.is_null() {
            return Err(unavailable(format!(
                "AudioConverterNew for 48 kHz stereo float32 → AAC failed (OSStatus {status})"
            )));
        }
        let mut owned = Self {
            converter,
            cookie: Vec::new(),
            fed_frames: 0,
        };
        // Bitrate is best-effort flavor, not validity: a refusal here must
        // not fail the encode, so it is deliberately unchecked beyond record.
        let mut rate = ENCODE_BIT_RATE;
        let rate_status = unsafe {
            AudioConverterSetProperty(
                converter,
                kAudioConverterEncodeBitRate,
                4,
                NonNull::new(&mut rate as *mut u32 as *mut c_void)
                    .expect("a stack rate is non-null"),
            )
        };
        if rate_status != 0 {
            tracing::warn!(
                "AAC bitrate property refused (OSStatus {rate_status}); continuing at codec default"
            );
        }
        owned.cookie = unsafe { read_cookie(converter)? };
        if owned.cookie.is_empty() {
            unsafe { AudioConverterDispose(converter) };
            return Err(unavailable(
                "AAC magic cookie came back empty; refusing to write a headerless stream",
            ));
        }
        Ok(owned)
    }

    /// The `AudioSpecificConfig` for the file's `esds` box.
    ///
    /// NOTE: despite the name, AudioToolbox does NOT return a bare 2-byte
    /// ASC here — the cookie is the full ES_Descriptor hierarchy (tag 0x03
    /// with extended-length encoding, verified by dump: 39 bytes for AAC-LC
    /// 48 kHz stereo, embedding the 0x05 DecoderSpecificInfo). Use
    /// [`Self::esds_content`] for the file; this accessor stays for tests.
    pub fn audio_specific_config(&self) -> &[u8] {
        &self.cookie
    }

    /// Bytes to place after the `esds` version/flags: the magic cookie
    /// verbatim when it is the ES_Descriptor hierarchy the codec produced
    /// (tag 0x03), minimally wrapped when a bare ASC ever arrives instead.
    /// Verbatim is the faithful path — bit-exact what the codec emitted,
    /// including the encode bitrate set at creation.
    pub fn esds_content(&self) -> Vec<u8> {
        if self.cookie.first() == Some(&0x03) {
            return self.cookie.clone();
        }
        // Bare ASC fallback: wrap in the minimal ES hierarchy.
        let asc = &self.cookie;
        let mut specific = vec![0x05];
        push_desc_len(&mut specific, asc.len());
        specific.extend_from_slice(asc);
        specific.push(0x06);
        push_desc_len(&mut specific, 1);
        specific.push(0x02);
        let mut config = vec![0x04];
        push_desc_len(&mut config, 1 + 1 + 3 + 4 + 4 + specific.len());
        config.push(0x40);
        config.push(0x15);
        config.extend_from_slice(&[0, 0, 0]);
        config.extend_from_slice(&ENCODE_BIT_RATE.to_be_bytes());
        config.extend_from_slice(&ENCODE_BIT_RATE.to_be_bytes());
        config.extend_from_slice(&specific);
        let mut es = vec![0x03];
        push_desc_len(&mut es, 2 + 1 + config.len());
        es.extend_from_slice(&[0, 2, 0]);
        es.extend_from_slice(&config);
        es
    }

    /// Frames fed so far (input timeline, pre-priming).
    pub fn fed_frames(&self) -> u64 {
        self.fed_frames
    }

    /// Encode interleaved f32 stereo frames. Returns complete AAC packets;
    /// the converter may hold back input internally (priming), which `flush`
    /// collects. `pcm.len()` must be even (stereo pairs).
    pub fn encode_interleaved_f32(&mut self, pcm: &[f32]) -> Result<Vec<AacFrame>, AacError> {
        if !pcm.len().is_multiple_of(INPUT_CHANNELS as usize) {
            return Err(unavailable(format!(
                "refusing {} samples: not whole stereo frames",
                pcm.len()
            )));
        }
        let frames = pcm.len() / INPUT_CHANNELS as usize;
        let out = self.fill(pcm.as_ptr(), frames, false)?;
        self.fed_frames += frames as u64;
        Ok(out)
    }

    /// Signal end-of-data and collect the remaining packets (including the
    /// internally padded final packet).
    pub fn flush(&mut self) -> Result<Vec<AacFrame>, AacError> {
        self.fill(std::ptr::null(), 0, true)
    }

    /// Run `FillComplexBuffer` until it stops producing: feed `frames` from
    /// `ptr` (null + 0 + `eos` means end-of-data immediately), collecting up
    /// to 32 packets per call.
    fn fill(
        &mut self,
        ptr: *const f32,
        frames: usize,
        eos: bool,
    ) -> Result<Vec<AacFrame>, AacError> {
        let mut state = FeedState {
            cursor: ptr,
            frames_left: frames,
            eos,
        };
        let mut frames_out = Vec::new();
        // Bounded: each iteration either consumes input or emits packets (or
        // both); the cap is a backstop against a misbehaving codec, not a
        // normal exit.
        for _ in 0..4096 {
            let mut out_buf = vec![0u8; 32 * 2048];
            let mut out_descs = vec![EMPTY_PACKET_DESC; 32];
            let mut n_packets: u32 = 32;
            let mut list = AudioBufferList {
                mNumberBuffers: 1,
                mBuffers: [AudioBuffer {
                    mNumberChannels: INPUT_CHANNELS,
                    mDataByteSize: out_buf.len() as u32,
                    mData: out_buf.as_mut_ptr() as *mut c_void,
                }],
            };
            let status = unsafe {
                AudioConverterFillComplexBuffer(
                    self.converter,
                    Some(input_proc),
                    &mut state as *mut FeedState as *mut c_void,
                    NonNull::from(&mut n_packets),
                    NonNull::from(&mut list),
                    out_descs.as_mut_ptr(),
                )
            };
            // Collect first: a drained-input stop still carries output.
            for desc in out_descs.iter().take(n_packets as usize) {
                let start = desc.mStartOffset as usize;
                let len = desc.mDataByteSize as usize;
                if start + len > list.mBuffers[0].mDataByteSize as usize {
                    return Err(unavailable(format!(
                        "AAC packet [{start}..{start}+{len}] runs past the {n} output bytes",
                        n = list.mBuffers[0].mDataByteSize,
                    )));
                }
                frames_out.push(AacFrame {
                    data: out_buf[start..start + len].to_vec(),
                });
            }
            if status == INPUT_DRAINED {
                // Mid-stream input exhaustion (only when !eos): the
                // converter holds its state; the caller feeds more later.
                break;
            }
            if status != 0 {
                return Err(unavailable(format!(
                    "AAC conversion failed (OSStatus {status})"
                )));
            }
            if n_packets == 0 {
                break;
            }
        }
        Ok(frames_out)
    }

    /// True when AudioToolbox answers a full probe: open, encode 1024 silent
    /// frames, flush, and see a real packet with a real cookie.
    pub fn is_available() -> bool {
        let mut enc = match Self::new() {
            Ok(e) => e,
            Err(_) => return false,
        };
        if enc.cookie.is_empty() {
            return false;
        }
        let silence = vec![0.0f32; FRAMES_PER_PACKET * INPUT_CHANNELS as usize];
        match enc.encode_interleaved_f32(&silence).and_then(|mut v| {
            v.extend(enc.flush()?);
            Ok(v)
        }) {
            Ok(frames) => !frames.is_empty() && frames.iter().all(|f| !f.data.is_empty()),
            Err(_) => false,
        }
    }
}

const EMPTY_PACKET_DESC: AudioStreamPacketDescription = AudioStreamPacketDescription {
    mStartOffset: 0,
    mVariableFramesInPacket: 0,
    mDataByteSize: 0,
};

/// MP4 descriptor length: base-128 big-endian, minimal bytes.
fn push_desc_len(buf: &mut Vec<u8>, len: usize) {
    let mut stack = [0u8; 5];
    let mut n = 0;
    let mut v = len;
    loop {
        stack[n] = (v & 0x7F) as u8;
        n += 1;
        v >>= 7;
        if v == 0 {
            break;
        }
    }
    for i in (0..n).rev() {
        let mut byte = stack[i];
        if i != 0 {
            byte |= 0x80;
        }
        buf.push(byte);
    }
}

/// Read the compression magic cookie (the `AudioSpecificConfig`).
unsafe fn read_cookie(converter: AudioConverterRef) -> Result<Vec<u8>, AacError> {
    unsafe {
        let mut size: u32 = 0;
        let info_status = AudioConverterGetPropertyInfo(
            converter,
            kAudioConverterCompressionMagicCookie,
            &mut size,
            std::ptr::null_mut(),
        );
        if info_status != 0 || size == 0 || size > 256 {
            return Err(unavailable(format!(
                "AAC magic cookie query failed (OSStatus {info_status}, size {size})"
            )));
        }
        let mut cookie = vec![0u8; size as usize];
        let mut got = size;
        let get_status = AudioConverterGetProperty(
            converter,
            kAudioConverterCompressionMagicCookie,
            NonNull::from(&mut got),
            NonNull::new(cookie.as_mut_ptr() as *mut c_void).expect("cookie buffer is non-null"),
        );
        if get_status != 0 {
            return Err(unavailable(format!(
                "AAC magic cookie read failed (OSStatus {get_status})"
            )));
        }
        cookie.truncate(got as usize);
        Ok(cookie)
    }
}

impl Drop for AacEncoder {
    fn drop(&mut self) {
        unsafe {
            AudioConverterDispose(self.converter);
        }
    }
}

// `converter` is owned by this struct and only touched from the thread that
// owns it (the input proc runs synchronously inside `FillComplexBuffer`).
// The raw pointer therefore never crosses threads; these impls are absent
// deliberately — an `AacEncoder` that is `Send` would be a lie.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_encodes_a_real_packet_with_a_cookie() {
        assert!(
            AacEncoder::is_available(),
            "LOUD FAILURE: AudioToolbox AAC refused the probe — no silent fallback exists"
        );
        let mut enc = AacEncoder::new().expect("probe passed so open must succeed");
        assert!(
            !enc.audio_specific_config().is_empty(),
            "esds needs the AudioSpecificConfig"
        );
        // Shape contract the writer relies on: the cookie is the full ES
        // hierarchy, so esds_content is verbatim. If Apple ever ships a bare
        // ASC, the fallback wrap covers it — this assertion pins the current
        // shape so a change fails loudly here, not as a corrupt file later.
        assert_eq!(
            enc.audio_specific_config()[0],
            0x03,
            "AAC magic cookie must open with the ES_Descriptor tag"
        );
        assert!(!enc.esds_content().is_empty());
        // 440 Hz tone, one packet worth: the output must be non-trivial bytes,
        // proving the frames carry signal rather than container padding.
        let mut tone = Vec::with_capacity(FRAMES_PER_PACKET * 2);
        for n in 0..FRAMES_PER_PACKET {
            let v = 0.5 * (2.0 * std::f32::consts::PI * 440.0 * n as f32 / 48_000.0).sin();
            tone.push(v);
            tone.push(v);
        }
        let mut frames = enc.encode_interleaved_f32(&tone).expect("tone must encode");
        frames.extend(enc.flush().expect("flush must succeed"));
        assert!(!frames.is_empty(), "a packet in must yield packets out");
        assert!(frames.iter().all(|f| !f.data.is_empty()));
    }

    #[test]
    fn odd_sample_count_is_refused() {
        let mut enc = AacEncoder::new().expect("open must succeed");
        let err = enc
            .encode_interleaved_f32(&[0.0, 0.0, 0.0])
            .expect_err("3 samples are not whole stereo frames");
        assert_eq!(err.kind_token(), "E_AAC_UNAVAILABLE");
    }
}
