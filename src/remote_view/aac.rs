//! Optional worker-local AAC-LC encoding through Windows' built-in MFT.
use super::video::Runtime;
use std::{mem::ManuallyDrop, rc::Rc};
use windows::Win32::{Media::MediaFoundation::*, System::Com::*};
use windows_core::{Result, GUID};

pub(super) struct Encoder {
    transform: IMFTransform,
    output: IMFSample,
    frames: i64,
    runtime: Rc<Runtime>,
}

impl Encoder {
    pub(super) fn new(runtime: &Rc<Runtime>) -> Result<Self> {
        unsafe {
            // CLSID_AACMFTEncoder, instantiated only on an explicit audio request.
            let transform: IMFTransform = CoCreateInstance(
                &GUID::from_u128(0x93af0c51_2275_45d2_a35b_f2ba21caed00),
                None,
                CLSCTX_INPROC_SERVER,
            )?;
            for input in [true, false] {
                let ty = runtime.media_type()?;
                ty.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
                ty.SetGUID(
                    &MF_MT_SUBTYPE,
                    &if input {
                        MFAudioFormat_PCM
                    } else {
                        MFAudioFormat_AAC
                    },
                )?;
                ty.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, 2)?;
                ty.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, 48_000)?;
                ty.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
                if input {
                    transform.SetInputType(0, &ty, 0)?;
                } else {
                    ty.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, 16_000)?;
                    ty.SetUINT32(&MF_MT_AAC_PAYLOAD_TYPE, 0)?;
                    transform.SetOutputType(0, &ty, 0)?;
                }
            }
            let info = transform.GetOutputStreamInfo(0)?;
            if info.cbSize > 16_384 {
                return Err(windows_core::Error::from_hresult(MF_E_INVALIDMEDIATYPE));
            }
            let output = runtime.sample()?;
            output.AddBuffer(&runtime.buffer(info.cbSize.max(8192))?)?;
            transform.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
            transform.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;
            Ok(Self {
                transform,
                output,
                frames: 0,
                runtime: runtime.clone(),
            })
        }
    }

    pub(super) fn encode(&mut self, pcm: &[i16; 2048]) -> Result<Option<Vec<u8>>> {
        unsafe {
            let buffer = self.runtime.buffer(4096)?;
            let mut data = std::ptr::null_mut();
            buffer.Lock(&mut data, None, None)?;
            std::ptr::copy_nonoverlapping(pcm.as_ptr() as *const u8, data, 4096);
            buffer.Unlock()?;
            buffer.SetCurrentLength(4096)?;
            let sample = self.runtime.sample()?;
            sample.AddBuffer(&buffer)?;
            sample.SetSampleTime(self.frames * 10_000_000 / 48_000)?;
            sample.SetSampleDuration(1024 * 10_000_000 / 48_000)?;
            self.frames += 1024;
            self.transform.ProcessInput(0, &sample, 0)?;
            self.output.GetBufferByIndex(0)?.SetCurrentLength(0)?;
            let mut outputs = [MFT_OUTPUT_DATA_BUFFER {
                pSample: ManuallyDrop::new(Some(self.output.clone())),
                ..Default::default()
            }];
            let mut status = 0;
            let result = self.transform.ProcessOutput(0, &mut outputs, &mut status);
            let sample = ManuallyDrop::take(&mut outputs[0].pSample);
            ManuallyDrop::drop(&mut outputs[0].pEvents);
            match result {
                Err(error) if error.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(None),
                other => other?,
            }
            let buffer = sample
                .ok_or_else(|| windows_core::Error::from_hresult(MF_E_UNEXPECTED))?
                .ConvertToContiguousBuffer()?;
            let mut bytes = std::ptr::null_mut();
            let mut length = 0;
            buffer.Lock(&mut bytes, None, Some(&mut length))?;
            let packet = if length > 0 && length <= 8192 && !bytes.is_null() {
                Some(std::slice::from_raw_parts(bytes, length as usize).to_vec())
            } else {
                None
            };
            buffer.Unlock()?;
            Ok(packet)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires the installed Windows AAC encoder"]
    fn native_aac_encodes_stereo_at_bounded_bitrate() {
        let runtime = Runtime::new().unwrap();
        let mut encoder = Encoder::new(&runtime).unwrap();
        let mut count = 0;
        let mut bytes = 0;
        let mut fixture = Vec::new();
        for frame in 0..100 {
            let mut pcm = [0; 2048];
            for i in 0..1024 {
                let tone = (((frame * 1024 + i) as f32 * std::f32::consts::TAU * 440.0 / 48_000.0)
                    .sin()
                    * 12000.0) as i16;
                pcm[i * 2] = tone;
                pcm[i * 2 + 1] = tone;
            }
            if let Some(packet) = encoder.encode(&pcm).unwrap() {
                count += 1;
                bytes += packet.len();
                let length = packet.len() + 7;
                fixture.extend_from_slice(&[
                    0xff,
                    0xf1,
                    0x4c,
                    0x80 | ((length >> 11) as u8),
                    (length >> 3) as u8,
                    ((length & 7) as u8) << 5 | 0x1f,
                    0xfc,
                ]);
                fixture.extend_from_slice(&packet);
            }
        }
        assert!(count >= 95, "only {count} packets");
        assert!(
            (1000..45_000).contains(&bytes),
            "unexpected bitrate: {bytes} bytes"
        );
        if let Some(path) = std::env::var_os("PLURIVIEW_TEST_AAC") {
            std::fs::write(path, fixture).unwrap();
        }
    }
}
