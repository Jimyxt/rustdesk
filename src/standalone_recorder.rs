use std::sync::{Arc, Mutex};
use std::thread;

use hbb_common::{log, ResultType};
use scrap::codec::{Encoder, EncoderCfg, EncoderApi};
use scrap::codec::record::{Recorder, RecorderContext};
use scrap::vpxcodec::{VpxEncoderConfig, VpxVideoCodecId};
use scrap::{Capturer, Display, TraitCapturer, STRIDE_ALIGN};

use crate::ui_interface::video_save_directory;

/// Standalone screen recorder that works without a remote connection.
/// Captures the local screen, encodes with VP9, and writes to a .webm file.
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
    let mut capturer = Capturer::new(d).with_context(|| "Failed to create capturer")?;

    // Create VP9 encoder
    let encoder_cfg = EncoderCfg::VPX(VpxEncoderConfig {
        width: width as _,
        height: height as _,
        quality: 1.0,
        codec: VpxVideoCodecId::VP9,
        keyframe_interval: Some(240),
    });
    let mut encoder = Encoder::new(encoder_cfg);

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

    // Capture + encode + record loop
    let mut ms = 0i64;
    let frame_interval = std::time::Duration::from_millis(33); // ~30fps

    while *recording.lock().unwrap() {
        let start = std::time::Instant::now();

        // Capture frame
        match capturer.frame() {
            Ok(frame) => {
                let yuv = scrap::common::convert::ARGBToYUV(
                    &frame,
                    width,
                    height,
                    STRIDE_ALIGN,
                );
                if let Ok(encoded) = encoder.encode(&yuv, ms) {
                    for frame in encoded {
                        let mut msg = hbb_common::message_proto::Message::new();
                        let mut vf = hbb_common::message_proto::VideoFrame::new();
                        let mut vp9s = hbb_common::message_proto::Vpxs::new();
                        let mut vpx_frame = hbb_common::message_proto::EncodedVideoFrame::new();
                        vpx_frame.data = frame.data.into();
                        vpx_frame.key = frame.key;
                        vpx_frame.pts = frame.pts;
                        vp9s.frames.push(vpx_frame);
                        vf.set_vp9s(vp9s);
                        msg.set_video_frame(vf);
                        if let Some(r) = recorder.lock().unwrap().as_mut() {
                            r.write_message(&msg, width, height);
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