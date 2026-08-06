use std::sync::{Arc, Mutex};
use std::thread;

use hbb_common::{log, message_proto::Message, ResultType};
use scrap::codec::{Encoder, EncoderCfg, EncoderApi};
use scrap::record::{Recorder, RecorderContext};
use scrap::vpxcodec::{VpxEncoderConfig, VpxVideoCodecId};
use scrap::{Capturer, Display, TraitCapturer};

use crate::ui_interface::video_save_directory;

/// Standalone screen recorder that works without a remote connection.
/// Captures the local screen, encodes with VP9, and writes to a .webm file.
/// Also captures local system audio (loopback) and writes an Opus track to the
/// same .webm via the shared `Recorder`. Audio capture runs in a sibling thread
/// managed by `crate::standalone_audio`; see that module for platform coverage.
pub struct StandaloneRecorder {
    recording: Arc<Mutex<bool>>,
    thread_handle: Option<thread::JoinHandle<()>>,
}

impl Default for StandaloneRecorder {
    fn default() -> Self {
        Self {
            recording: Arc::new(Mutex::new(false)),
            thread_handle: None,
        }
    }
}

impl StandaloneRecorder {
    /// Start recording the primary display.
    pub fn start(&mut self) -> ResultType<()> {
        let recording = self.recording.clone();
        *recording.lock().unwrap() = true;

        let handle = thread::spawn(move || {
            if let Err(e) = run_recording_loop(recording.clone()) {
                log::error!("Standalone recording error: {e:?}");
            }
            *recording.lock().unwrap() = false;
        });

        self.thread_handle = Some(handle);
        Ok(())
    }

    /// Stop recording.
    pub fn stop(&mut self) {
        *self.recording.lock().unwrap() = false;
        if let Some(handle) = self.thread_handle.take() {
            let _ = handle.join();
        }
    }

    /// Check if currently recording.
    pub fn is_recording(&self) -> bool {
        *self.recording.lock().unwrap()
    }
}

fn run_recording_loop(recording: Arc<Mutex<bool>>) -> ResultType<()> {
    // Get primary display
    let d = Display::primary()?;
    let (width, height) = (d.width(), d.height());
    log::info!(
        "Standalone recording started: {}x{}",
        width,
        height
    );

    // Create capturer
    let mut capturer = Capturer::new(d)?;

    // Create VP9 encoder
    let encoder_cfg = EncoderCfg::VPX(VpxEncoderConfig {
        width: width as _,
        height: height as _,
        quality: 1.0,
        codec: VpxVideoCodecId::VP9,
        keyframe_interval: Some(240),
    });
    let mut encoder = Encoder::new(encoder_cfg, false)?;

    // Allocate YUV conversion buffers
    let yuvfmt = encoder.yuvfmt();
    let mut yuv_buf: Vec<u8> = Vec::new();
    let mut mid_data: Vec<u8> = Vec::new();

    // Create recorder (file writer)
    let save_dir = video_save_directory(false);
    let recorder = Recorder::new(RecorderContext {
        server: false,
        id: "standalone".to_owned(),
        dir: save_dir,
        display_idx: 0,
        camera: false,
        tx: None,
    })?;
    let recorder = Arc::new(Mutex::new(Some(recorder)));

    // Start audio capture in a sibling thread sharing the same recorder.
    let audio_handle =
        crate::standalone_audio::start_audio_capture(recorder.clone(), recording.clone());

    // Capture + encode + record loop
    let mut ms = 0i64;
    let frame_interval = std::time::Duration::from_millis(33); // ~30fps

    while *recording.lock().unwrap() {
        let start = std::time::Instant::now();

        // Capture frame (pass the frame interval as the timeout)
        match capturer.frame(frame_interval) {
            Ok(frame) => {
                if frame.valid() {
                    match frame.to(yuvfmt.clone(), &mut yuv_buf, &mut mid_data) {
                        Ok(input) => {
                            match encoder.encode_to_message(input, ms) {
                                Ok(vf) => {
                                    let mut msg = Message::new();
                                    msg.set_video_frame(vf);
                                    if let Some(r) = recorder.lock().unwrap().as_mut() {
                                        r.write_message(&msg, width, height);
                                    }
                                }
                                Err(e) => {
                                    log::error!("Encode error: {e:?}");
                                }
                            }
                        }
                        Err(e) => {
                            log::error!("YUV conversion error: {e:?}");
                        }
                    }
                }
                ms += 33;
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                // No frame available yet
            }
            Err(e) => {
                log::error!("Capture error: {e:?}");
                break;
            }
        }

        // Sleep to maintain frame rate
        let elapsed = start.elapsed();
        if elapsed < frame_interval {
            thread::sleep(frame_interval - elapsed);
        }
    }

    // Stop audio capture first (sets recording=false above already signals it),
    // then drop the recorder so the WebmRecorder finalizes the .webm with the
    // audio track flushed.
    let _ = audio_handle.join();
    drop(recorder);
    log::info!("Standalone recording stopped");
    Ok(())
}

use std::sync::OnceLock;

static STANDALONE_RECORDER: OnceLock<Mutex<StandaloneRecorder>> = OnceLock::new();

/// Get the global standalone recorder instance.
pub fn standalone_recorder() -> &'static Mutex<StandaloneRecorder> {
    STANDALONE_RECORDER.get_or_init(|| Mutex::new(StandaloneRecorder::default()))
}

/// Start standalone recording. Returns true on success.
pub fn start_standalone_recording() -> bool {
    match standalone_recorder().lock() {
        Ok(mut r) => r.start().is_ok(),
        Err(_) => false,
    }
}

/// Stop standalone recording.
pub fn stop_standalone_recording() {
    if let Ok(mut r) = standalone_recorder().lock() {
        r.stop();
    }
}

/// Check if standalone recording is active.
pub fn is_standalone_recording() -> bool {
    if let Ok(r) = standalone_recorder().lock() {
        r.is_recording()
    } else {
        false
    }
}