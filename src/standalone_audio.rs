// Standalone system-audio capture for the fork's standalone screen recorder.
//
// Mirrors the capture+encode pattern of `src/server/audio_service.rs` but, instead
// of sending Opus packets over the network via `GenericService`, it writes them to
// the shared `Recorder` (the same `Recorder` the video loop writes to) through
// `Recorder::write_audio`. This keeps the host->peer streaming path untouched.
//
// The encoded Opus stream is 48 kHz stereo 10 ms, matching the WebmRecorder audio
// track created unconditionally in `libs/scrap/src/common/record.rs` (Opus 48000/2).
//
// Platform coverage:
//   - Windows: cpal `default_output_device()` (WASAPI loopback).
//   - macOS: cpal `ScreenCaptureKit` host when the `screencapturekit` feature is on
//     (system loopback); otherwise `default_input_device()` (mic fallback, same
//     limitation the host streaming has today).
//   - Linux: PulseAudio `psimple::Simple` record on the monitor source.
//   - Android/other: no-op (video-only) — standalone recording isn't supported there.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use hbb_common::{bail, log, ResultType};
use magnum_opus::{Application::*, Channels::*, Encoder};
use scrap::record::Recorder;

/// Opus track the WebmRecorder creates is 48 kHz stereo. Encode to match.
const ENC_SAMPLE_RATE: u32 = 48_000;
const ENC_CHANNELS: u16 = 2;
/// Each Opus frame is 10 ms (`audio_service.rs`: `frame_size = sample_rate / 100`).
const FRAME_MS: i64 = 10;

/// Start a background thread that captures system audio and writes Opus frames
/// to `recorder`. The thread stops when `recording` goes false. The returned
/// handle lets the caller join the audio thread when stopping.
pub fn start_audio_capture(
    recorder: Arc<Mutex<Option<Recorder>>>,
    recording: Arc<Mutex<bool>>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        if let Err(e) = run(&recorder, &recording) {
            log::error!("Standalone audio capture error: {e:?}");
        }
    })
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
fn run(
    recorder: &Arc<Mutex<Option<Recorder>>>,
    recording: &Arc<Mutex<bool>>,
) -> ResultType<()> {
    use cpal::traits::{HostTrait, StreamTrait};
    use cpal::{BufferSize, SampleFormat, StreamConfig};

    #[cfg(feature = "screencapturekit")]
    let host: cpal::Host = if cpal::available_hosts()
        .iter()
        .any(|h| *h == cpal::HostId::ScreenCaptureKit)
    {
        cpal::host_from_id(cpal::HostId::ScreenCaptureKit)
            .unwrap_or_else(|_| cpal::default_host())
    } else {
        cpal::default_host()
    };
    #[cfg(not(feature = "screencapturekit"))]
    let host: cpal::Host = cpal::default_host();

    #[cfg(windows)]
    let (device, config) = get_device_win(&host)?;
    #[cfg(not(windows))]
    let (device, config) = get_device_input(&host)?;

    let sample_rate_0 = config.sample_rate().0;
    let device_channel = config.channels();
    let stream = match config.sample_format() {
        SampleFormat::I8 => build_input_stream::<i8>(device, &config, recorder.clone(), sample_rate_0, device_channel)?,
        SampleFormat::I16 => build_input_stream::<i16>(device, &config, recorder.clone(), sample_rate_0, device_channel)?,
        SampleFormat::I32 => build_input_stream::<i32>(device, &config, recorder.clone(), sample_rate_0, device_channel)?,
        SampleFormat::I64 => build_input_stream::<i64>(device, &config, recorder.clone(), sample_rate_0, device_channel)?,
        SampleFormat::U8 => build_input_stream::<u8>(device, &config, recorder.clone(), sample_rate_0, device_channel)?,
        SampleFormat::U16 => build_input_stream::<u16>(device, &config, recorder.clone(), sample_rate_0, device_channel)?,
        SampleFormat::U32 => build_input_stream::<u32>(device, &config, recorder.clone(), sample_rate_0, device_channel)?,
        SampleFormat::U64 => build_input_stream::<u64>(device, &config, recorder.clone(), sample_rate_0, device_channel)?,
        SampleFormat::F32 => build_input_stream::<f32>(device, &config, recorder.clone(), sample_rate_0, device_channel)?,
        SampleFormat::F64 => build_input_stream::<f64>(device, &config, recorder.clone(), sample_rate_0, device_channel)?,
        f => bail!("unsupported audio format: {:?}", f),
    };
    stream.play()?;

    // Keep the stream alive while recording; drop it (which stops capture) on stop.
    while *recording.lock().unwrap() {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    drop(stream);
    log::info!("Standalone audio capture stopped");
    Ok(())
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
#[cfg(windows)]
fn get_device_win(host: &cpal::Host) -> ResultType<(cpal::Device, cpal::SupportedStreamConfig)> {
    use cpal::traits::{DeviceTrait, HostTrait};

    let device = host
        .default_output_device()
        .with_context(|| "Failed to get default output device for loopback")?;
    log::info!(
        "Standalone audio device: {}",
        device.name().unwrap_or("".to_owned())
    );
    let config = device
        .default_output_config()
        .map_err(|e| anyhow!(e))
        .with_context(|| "Failed to get default output format")?;
    Ok((device, config))
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
#[cfg(not(windows))]
fn get_device_input(host: &cpal::Host) -> ResultType<(cpal::Device, cpal::SupportedStreamConfig)> {
    use cpal::traits::{DeviceTrait, HostTrait};

    let device = host
        .default_input_device()
        .with_context(|| "Failed to get default input device for loopback")?;
    log::info!(
        "Standalone audio device: {}",
        device.name().unwrap_or("".to_owned())
    );
    let config = device
        .default_input_config()
        .map_err(|e| anyhow!(e))
        .with_context(|| "Failed to get default input format")?;
    Ok((device, config))
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
fn build_input_stream<T>(
    device: cpal::Device,
    config: &cpal::SupportedStreamConfig,
    recorder: Arc<Mutex<Option<Recorder>>>,
    sample_rate_0: u32,
    device_channel: u16,
) -> ResultType<cpal::Stream>
where
    T: cpal::SizedSample + dasp::sample::ToSample<f32>,
{
    use cpal::{BufferSize, StreamConfig, traits::DeviceTrait};

    let err_fn = |err| log::trace!("an error occurred on the standalone audio stream: {}", err);
    let encode_channel = if ENC_CHANNELS > 1 { Stereo } else { Mono };
    // 10 ms at the device rate, same sizing as `audio_service.rs`.
    let frame_size = sample_rate_0 as usize / 100;
    let encode_len = frame_size * ENC_CHANNELS as usize;
    let rechannel_len = encode_len * device_channel as usize / ENC_CHANNELS as usize;
    let mut encoder = Encoder::new(ENC_SAMPLE_RATE, encode_channel, LowDelay)?;
    let mut buffer: VecDeque<f32> = VecDeque::with_capacity(rechannel_len * 2);
    let pts = Arc::new(AtomicI64::new(0));
    let stream_config = StreamConfig {
        channels: device_channel,
        sample_rate: config.sample_rate(),
        buffer_size: BufferSize::Default,
    };
    let stream = device.build_input_stream(
        &stream_config,
        move |data: &[T], _: &cpal::InputCallbackInfo| {
            for s in data {
                buffer.push_back(T::to_sample(*s));
            }
            while buffer.len() >= rechannel_len {
                let frame: Vec<f32> = buffer.drain(0..rechannel_len).collect();
                // Resample device rate -> 48 kHz (keeps device_channel).
                let frame = if sample_rate_0 != ENC_SAMPLE_RATE {
                    crate::common::audio_resample(&frame, sample_rate_0, ENC_SAMPLE_RATE, device_channel)
                } else {
                    frame
                };
                // Rechannel device_channel -> 2 (stereo).
                let frame = if device_channel != ENC_CHANNELS {
                    crate::common::audio_rechannel(
                        frame,
                        ENC_SAMPLE_RATE,
                        ENC_SAMPLE_RATE,
                        device_channel,
                        ENC_CHANNELS,
                    )
                } else {
                    frame
                };
                match encoder.encode_vec_float(&frame, frame.len() * 6) {
                    Ok(opus) => {
                        let p = pts.fetch_add(FRAME_MS, Ordering::SeqCst);
                        if let Some(r) = recorder.lock().unwrap().as_mut() {
                            r.write_audio(&opus, p);
                        }
                    }
                    Err(_) => {}
                }
            }
        },
        err_fn,
        None,
    )?;
    Ok(stream)
}

#[cfg(target_os = "linux")]
fn run(
    recorder: &Arc<Mutex<Option<Recorder>>>,
    recording: &Arc<Mutex<bool>>,
) -> ResultType<()> {
    use crate::audio_service::AUDIO_DATA_SIZE_U8;

    let device = crate::platform::linux::get_pa_monitor();
    if device.is_empty() {
        log::warn!("Standalone audio: no PulseAudio monitor device; recording video-only");
        return Ok(());
    }
    let spec = pulse::sample::Spec {
        format: pulse::sample::Format::F32le,
        channels: 2,
        rate: crate::platform::PA_SAMPLE_RATE,
    };
    log::info!("Standalone audio pa monitor: {:?}", device);
    // Vec<f32> is 4-byte aligned, so reading F32le bytes into it and reinterpreting
    // as f32 is sound (unlike a Vec<u8>, whose alignment is 1 and would need the
    // `align_to_32_if_needed` dance `audio_service.rs` does).
    let mut samples: Vec<f32> = vec![0.0; AUDIO_DATA_SIZE_U8 / 4];
    let s = match psimple::Simple::new(
        None,
        &crate::get_app_name(),
        pulse::stream::Direction::Record,
        Some(&device),
        "record",
        &spec,
        None,
        None,
    ) {
        Ok(s) => s,
        Err(e) => {
            log::warn!("Standalone audio: could not create PulseAudio simple: {}", e);
            return Ok(());
        }
    };
    let mut encoder = Encoder::new(ENC_SAMPLE_RATE, Stereo, LowDelay)?;
    let mut pts: i64 = 0;
    while *recording.lock().unwrap() {
        // SAFETY: `samples` owns AUDIO_DATA_SIZE_U8 bytes backed by a Vec<f32>
        // (align 4, valid for the lifetime of the borrow).
        let buf_bytes: &mut [u8] = unsafe {
            std::slice::from_raw_parts_mut(
                samples.as_mut_ptr() as *mut u8,
                AUDIO_DATA_SIZE_U8,
            )
        };
        if s.read(buf_bytes).is_err() {
            continue;
        }
        // Skip all-zero (silence) frames, mirroring `ipc::start_pa`.
        if samples.iter().all(|&x| x == 0.0) {
            continue;
        }
        match encoder.encode_vec_float(&samples, samples.len() * 6) {
            Ok(opus) => {
                if let Some(r) = recorder.lock().unwrap().as_mut() {
                    r.write_audio(&opus, pts);
                }
                pts += FRAME_MS;
            }
            Err(_) => {}
        }
    }
    log::info!("Standalone audio capture stopped");
    Ok(())
}

#[cfg(target_os = "android")]
fn run(
    _recorder: &Arc<Mutex<Option<Recorder>>>,
    _recording: &Arc<Mutex<bool>>,
) -> ResultType<()> {
    // Standalone recording isn't supported on Android; video-only is fine.
    Ok(())
}

// Bring `anyhow!` and `Context::with_context` into scope for the cpal error mapping,
// mirroring `src/server/audio_service.rs`.
#[cfg(not(any(target_os = "linux", target_os = "android")))]
use hbb_common::anyhow::{anyhow, Context};