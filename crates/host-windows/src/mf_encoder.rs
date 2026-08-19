use std::mem::ManuallyDrop;
use std::thread;
use std::time::{Duration, Instant};

use nfidb_core::VideoCodec;
use windows::Win32::Media::MediaFoundation::{
    IMFActivate, IMFMediaEventGenerator, IMFTransform, METransformHaveOutput, METransformNeedInput,
    MF_E_NO_EVENTS_AVAILABLE, MF_EVENT_FLAG_NO_WAIT, MF_LOW_LATENCY, MFCreateMemoryBuffer, MFCreateSample,
    MFMediaType_Video, MFSampleExtension_CleanPoint, MFT_CATEGORY_VIDEO_ENCODER, MFT_ENUM_FLAG, MFT_ENUM_FLAG_ASYNCMFT,
    MFT_ENUM_FLAG_HARDWARE, MFT_ENUM_FLAG_LOCALMFT, MFT_ENUM_FLAG_SORTANDFILTER, MFT_ENUM_FLAG_SYNCMFT,
    MFT_FRIENDLY_NAME_Attribute, MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, MFT_MESSAGE_NOTIFY_START_OF_STREAM,
    MFT_OUTPUT_DATA_BUFFER, MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES, MFT_OUTPUT_STREAM_PROVIDES_SAMPLES,
    MFT_REGISTER_TYPE_INFO,
};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::core::Interface;

use crate::hardware::{attribute_string, codec_subtype, configure_probe, initialize_media_foundation};

pub struct MediaFoundationEncoder {
    transform: IMFTransform,
    events: IMFMediaEventGenerator,
    output_provides_samples: bool,
    output_buffer_size: u32,
    frame_duration_100ns: i64,
    next_timestamp_100ns: i64,
    pub encoder_name: String,
    pub codec: VideoCodec,
}

pub struct HardwareEncodedFrame {
    pub data: Vec<u8>,
    pub keyframe: bool,
}

impl MediaFoundationEncoder {
    pub fn new(codec: VideoCodec, width: u32, height: u32, fps: u32, bitrate: u32) -> Result<Self, String> {
        initialize_media_foundation()?;
        let activation = first_activation(codec)?;
        let encoder_name = attribute_string(&activation, &MFT_FRIENDLY_NAME_Attribute)
            .unwrap_or_else(|| format!("{} hardware encoder", codec.label()));
        let transform = unsafe { activation.ActivateObject::<IMFTransform>() }
            .map_err(|error| format!("activate {encoder_name}: {error}"))?;
        if let Ok(attributes) = unsafe { transform.GetAttributes() } {
            let _ = unsafe { attributes.SetUINT32(&MF_LOW_LATENCY, 1) };
        }
        configure_probe(&transform, codec, width, height, fps, bitrate)?;
        let stream_info = unsafe { transform.GetOutputStreamInfo(0) }
            .map_err(|error| format!("query {encoder_name} output allocation: {error}"))?;
        let events = transform
            .cast::<IMFMediaEventGenerator>()
            .map_err(|error| format!("{encoder_name} is not an asynchronous hardware MFT: {error}"))?;
        unsafe {
            transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)
                .map_err(|error| format!("begin {encoder_name} streaming: {error}"))?;
            transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)
                .map_err(|error| format!("start {encoder_name} stream: {error}"))?;
        }
        Ok(Self {
            transform,
            events,
            output_provides_samples: stream_info.dwFlags
                & (MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32 | MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES.0 as u32)
                != 0,
            output_buffer_size: stream_info.cbSize.max(width.saturating_mul(height)),
            frame_duration_100ns: 10_000_000_i64 / i64::from(fps.max(1)),
            next_timestamp_100ns: 0,
            encoder_name,
            codec,
        })
    }

    pub fn encode_nv12(&mut self, nv12: &[u8]) -> Result<HardwareEncodedFrame, String> {
        let timestamp = self.next_timestamp_100ns;
        self.next_timestamp_100ns = self.next_timestamp_100ns.saturating_add(self.frame_duration_100ns);
        let input = unsafe { MFCreateSample() }.map_err(|error| format!("create input sample: {error}"))?;
        let buffer = unsafe { MFCreateMemoryBuffer(nv12.len() as u32) }
            .map_err(|error| format!("allocate NV12 input buffer: {error}"))?;
        let mut destination = std::ptr::null_mut();
        unsafe {
            buffer
                .Lock(&mut destination, None, None)
                .map_err(|error| format!("lock NV12 input buffer: {error}"))?;
            std::ptr::copy_nonoverlapping(nv12.as_ptr(), destination, nv12.len());
            buffer
                .Unlock()
                .map_err(|error| format!("unlock NV12 input buffer: {error}"))?;
            buffer
                .SetCurrentLength(nv12.len() as u32)
                .map_err(|error| format!("set NV12 input length: {error}"))?;
            input
                .AddBuffer(&buffer)
                .map_err(|error| format!("attach NV12 input buffer: {error}"))?;
            input
                .SetSampleTime(timestamp)
                .map_err(|error| format!("timestamp input sample: {error}"))?;
            input
                .SetSampleDuration(self.frame_duration_100ns)
                .map_err(|error| format!("set input sample duration: {error}"))?;
        }

        let deadline = Instant::now() + Duration::from_millis(250);
        let mut input = Some(input);
        loop {
            if Instant::now() >= deadline {
                return Err(format!(
                    "{} timed out waiting for an asynchronous MFT event",
                    self.encoder_name
                ));
            }
            match unsafe { self.events.GetEvent(MF_EVENT_FLAG_NO_WAIT) } {
                Ok(event) => {
                    let event_type =
                        unsafe { event.GetType() }.map_err(|error| format!("read encoder event type: {error}"))?;
                    let status =
                        unsafe { event.GetStatus() }.map_err(|error| format!("read encoder event status: {error}"))?;
                    if status.is_err() {
                        return Err(format!(
                            "{} reported event failure 0x{:08x}",
                            self.encoder_name, status.0 as u32
                        ));
                    }
                    if event_type == METransformNeedInput.0 as u32 {
                        if let Some(sample) = input.take() {
                            unsafe { self.transform.ProcessInput(0, &sample, 0) }
                                .map_err(|error| format!("submit NV12 frame to {}: {error}", self.encoder_name))?;
                        }
                    } else if event_type == METransformHaveOutput.0 as u32 {
                        return self.take_output();
                    }
                }
                Err(error) if error.code() == MF_E_NO_EVENTS_AVAILABLE => {
                    thread::sleep(Duration::from_micros(100));
                }
                Err(error) => return Err(format!("poll {} events: {error}", self.encoder_name)),
            }
        }
    }

    fn take_output(&self) -> Result<HardwareEncodedFrame, String> {
        let supplied_sample = if self.output_provides_samples {
            None
        } else {
            let sample = unsafe { MFCreateSample() }.map_err(|error| format!("create output sample: {error}"))?;
            let buffer = unsafe { MFCreateMemoryBuffer(self.output_buffer_size) }
                .map_err(|error| format!("allocate encoded output buffer: {error}"))?;
            unsafe { sample.AddBuffer(&buffer) }.map_err(|error| format!("attach encoded output buffer: {error}"))?;
            Some(sample)
        };
        let mut output = MFT_OUTPUT_DATA_BUFFER {
            dwStreamID: 0,
            pSample: ManuallyDrop::new(supplied_sample),
            dwStatus: 0,
            pEvents: ManuallyDrop::new(None),
        };
        let mut status = 0_u32;
        unsafe {
            self.transform
                .ProcessOutput(0, std::slice::from_mut(&mut output), &mut status)
        }
        .map_err(|error| format!("retrieve {} output: {error}", self.encoder_name))?;
        let sample = unsafe { ManuallyDrop::take(&mut output.pSample) }
            .ok_or_else(|| format!("{} returned an output event without a sample", self.encoder_name))?;
        let events = unsafe { ManuallyDrop::take(&mut output.pEvents) };
        drop(events);
        let keyframe = unsafe { sample.GetUINT32(&MFSampleExtension_CleanPoint).unwrap_or(0) != 0 };
        let buffer = unsafe { sample.ConvertToContiguousBuffer() }
            .map_err(|error| format!("coalesce {} output: {error}", self.encoder_name))?;
        let length = unsafe { buffer.GetCurrentLength() }
            .map_err(|error| format!("read {} output length: {error}", self.encoder_name))?;
        let mut source = std::ptr::null_mut();
        unsafe { buffer.Lock(&mut source, None, None) }
            .map_err(|error| format!("lock {} output: {error}", self.encoder_name))?;
        let data = unsafe { std::slice::from_raw_parts(source, length as usize) }.to_vec();
        unsafe { buffer.Unlock() }.map_err(|error| format!("unlock {} output: {error}", self.encoder_name))?;
        if data.is_empty() {
            return Err(format!("{} produced an empty encoded sample", self.encoder_name));
        }
        Ok(HardwareEncodedFrame { data, keyframe })
    }
}

fn first_activation(codec: VideoCodec) -> Result<IMFActivate, String> {
    let output = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: codec_subtype(codec),
    };
    let flags = MFT_ENUM_FLAG(
        MFT_ENUM_FLAG_SYNCMFT.0
            | MFT_ENUM_FLAG_ASYNCMFT.0
            | MFT_ENUM_FLAG_HARDWARE.0
            | MFT_ENUM_FLAG_LOCALMFT.0
            | MFT_ENUM_FLAG_SORTANDFILTER.0,
    );
    let mut raw = std::ptr::null_mut();
    let mut count = 0_u32;
    unsafe {
        windows::Win32::Media::MediaFoundation::MFTEnumEx(
            MFT_CATEGORY_VIDEO_ENCODER,
            flags,
            None,
            Some(&output),
            &mut raw,
            &mut count,
        )
        .map_err(|error| format!("enumerate {} hardware encoders: {error}", codec.label()))?;
    }
    if raw.is_null() || count == 0 {
        return Err(format!("no {} hardware encoder was returned by Windows", codec.label()));
    }
    let activation = unsafe {
        let activations = std::slice::from_raw_parts_mut(raw, count as usize);
        let first = activations[0]
            .take()
            .ok_or_else(|| "hardware encoder activation was null".to_owned())?;
        for item in activations.iter_mut().skip(1) {
            drop(item.take());
        }
        CoTaskMemFree(Some(raw.cast()));
        first
    };
    Ok(activation)
}

pub fn functional_probe(codec: VideoCodec) -> Result<(String, usize, bool), String> {
    let width = 1280_u32;
    let height = 720_u32;
    let mut encoder = MediaFoundationEncoder::new(codec, width, height, 60, 5_000_000)?;
    let name = encoder.encoder_name.clone();
    let mut nv12 = vec![128_u8; width as usize * height as usize * 3 / 2];
    nv12[..width as usize * height as usize].fill(16);
    let output = encoder.encode_nv12(&nv12)?;
    Ok((name, output.data.len(), output.keyframe))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hardware_candidates_return_real_encoded_bytes() {
        for codec in [VideoCodec::H264, VideoCodec::Hevc, VideoCodec::Av1] {
            match functional_probe(codec) {
                Ok((name, bytes, keyframe)) => {
                    println!("{}: {name}, {bytes} bytes, keyframe={keyframe}", codec.label());
                    assert!(bytes > 16);
                }
                Err(error) => println!("{} unavailable: {error}", codec.label()),
            }
        }
    }
}
