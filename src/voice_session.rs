use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use jni::objects::{GlobalRef, JObject};
use jni::JNIEnv;

use crate::{audio, engine};

// --- Optional auto-stop endpointing (same level heuristics as recog_service) --
/// Absolute smoothed level (0..1) that must be exceeded to count as speech.
const MIN_SPEECH_LEVEL: f32 = 0.12;
/// How far above the running noise floor a level must be to count as speech.
const SPEECH_MARGIN: f32 = 0.08;
/// Trailing silence after speech that triggers auto-stop.
const AUTO_STOP_SILENCE_MS: u64 = 2000;
/// If no speech is ever detected, auto-stop after this long.
const AUTO_STOP_NO_SPEECH_MS: u64 = 8000;

pub struct SendStream(#[allow(dead_code)] pub cpal::Stream);
unsafe impl Send for SendStream {}
unsafe impl Sync for SendStream {}

/// Speech/silence tracking shared between the audio callback and the
/// auto-stop monitor thread.
struct Endpointing {
    last_voice: Mutex<Instant>,
    noise_floor: Mutex<f32>,
    speech_started: AtomicBool,
}

pub struct VoiceSessionState {
    pub stream: Option<SendStream>,
    pub audio_buffer: Arc<Mutex<Vec<f32>>>,
    pub jvm: Arc<jni::JavaVM>,
    pub target_ref: GlobalRef,
    pub last_level_sent: Arc<Mutex<std::time::Instant>>,
    /// True while the current recording runs; flipped off on stop/cancel so
    /// the auto-stop monitor (if any) exits.
    pub session_active: Arc<AtomicBool>,
}

fn notify_status(env: &mut JNIEnv, obj: &JObject, msg: &str) {
    if let Ok(jmsg) = env.new_string(msg) {
        let _ = env.call_method(
            obj,
            "onStatusUpdate",
            "(Ljava/lang/String;)V",
            &[(&jmsg).into()],
        );
    }
}

fn notify_level(env: &mut JNIEnv, obj: &JObject, level: f32) {
    let _ = env.call_method(obj, "onAudioLevel", "(F)V", &[level.into()]);
}

fn notify_text(env: &mut JNIEnv, obj: &JObject, text: &str) {
    if let Ok(jtxt) = env.new_string(text) {
        let _ = env.call_method(
            obj,
            "onTextTranscribed",
            "(Ljava/lang/String;)V",
            &[(&jtxt).into()],
        );
    }
}

pub fn init_session(env: JNIEnv, target: JObject) -> VoiceSessionState {
    android_logger::init_once(
        android_logger::Config::default().with_max_level(log::LevelFilter::Info),
    );

    let vm = env.get_java_vm().expect("Failed to get JavaVM");
    let vm_arc = Arc::new(vm);
    let target_ref = env.new_global_ref(&target).expect("Failed to ref target");

    let state = VoiceSessionState {
        stream: None,
        audio_buffer: Arc::new(Mutex::new(Vec::new())),
        jvm: vm_arc.clone(),
        target_ref: target_ref.clone(),
        last_level_sent: Arc::new(Mutex::new(std::time::Instant::now())),
        session_active: Arc::new(AtomicBool::new(false)),
    };

    // Load engine in background
    let vm_clone = vm_arc.clone();
    let target_ref_clone = target_ref.clone();

    std::thread::spawn(move || {
        let _ = engine::ensure_loaded_from_thread(&vm_clone, &target_ref_clone);
    });

    state
}

/// Begin microphone capture. With `auto_stop` set, a monitor thread watches
/// for trailing silence after speech (or a no-speech timeout) and invokes the
/// Java-side `onAutoStop()` callback, which is expected to stop the recording
/// the same way a manual tap would.
pub fn start_recording(mut env: JNIEnv, state: &mut VoiceSessionState, auto_stop: bool) {
    // Always stop a previous stream before attempting a new recorder.
    state.session_active.store(false, Ordering::SeqCst);
    state.stream = None;

    let host = cpal::default_host();
    let device = match host.default_input_device() {
        Some(d) => d,
        None => {
            notify_status(
                &mut env,
                state.target_ref.as_obj(),
                "Error: no microphone available. Check permissions.",
            );
            return;
        }
    };

    let input_format = match audio::select_input_format(&device) {
        Ok(format) => format,
        Err(_) => {
            log::error!("audio recorder config selection failed");
            notify_status(
                &mut env,
                state.target_ref.as_obj(),
                "Error: microphone recorder unavailable.",
            );
            return;
        }
    };

    state.audio_buffer.lock().unwrap().clear();
    let buffer_clone = state.audio_buffer.clone();

    // Arm a fresh flag after ending any previous session above.
    let session_active = Arc::new(AtomicBool::new(true));
    state.session_active = session_active.clone();

    let endpoint = if auto_stop {
        Some(Arc::new(Endpointing {
            last_voice: Mutex::new(Instant::now()),
            noise_floor: Mutex::new(0.0),
            speech_started: AtomicBool::new(false),
        }))
    } else {
        None
    };

    let jvm = state.jvm.clone();
    let target_ref = state.target_ref.clone();
    let last_sent = state.last_level_sent.clone();
    let endpoint_cb = endpoint.clone();

    let converter = Arc::new(Mutex::new(audio::CaptureConverter::new(
        input_format.config.channels,
        input_format.config.sample_rate.0,
    )));
    let runtime_active = session_active.clone();
    let runtime_jvm = state.jvm.clone();
    let runtime_target = state.target_ref.clone();
    let make_error = || {
        let active = runtime_active.clone();
        let callback_jvm = runtime_jvm.clone();
        let callback_target = runtime_target.clone();
        move |_| {
            log::error!("audio recorder runtime failure");
            active.store(false, Ordering::SeqCst);
            if let Ok(mut callback_env) = callback_jvm.attach_current_thread() {
                notify_status(
                    &mut callback_env,
                    callback_target.as_obj(),
                    "Error: microphone recorder stopped.",
                );
            }
        }
    };
    let make_callback = |buffer: Arc<Mutex<Vec<f32>>>,
                         jvm: Arc<jni::JavaVM>,
                         target_ref: GlobalRef,
                         last_sent: Arc<Mutex<Instant>>,
                         endpoint: Option<Arc<Endpointing>>| {
        move |samples: Vec<f32>| {
            if samples.is_empty() {
                return;
            }
            buffer.lock().unwrap().extend_from_slice(&samples);
            let rms = (samples.iter().map(|x| x * x).sum::<f32>() / samples.len() as f32).sqrt();
            let level = (rms * 6.0).clamp(0.0, 1.0);
            if let Some(ep) = &endpoint {
                let floor = *ep.noise_floor.lock().unwrap();
                if level > MIN_SPEECH_LEVEL && level > floor + SPEECH_MARGIN {
                    *ep.last_voice.lock().unwrap() = Instant::now();
                    ep.speech_started.store(true, Ordering::SeqCst);
                } else {
                    let mut nf = ep.noise_floor.lock().unwrap();
                    *nf = *nf * 0.95 + level * 0.05;
                }
            }
            let mut last = last_sent.lock().unwrap();
            if last.elapsed() >= Duration::from_millis(50) {
                *last = Instant::now();
                drop(last);
                if let Ok(mut callback_env) = jvm.attach_current_thread() {
                    notify_level(&mut callback_env, target_ref.as_obj(), level);
                }
            }
        }
    };
    let stream = match input_format.sample_format {
        cpal::SampleFormat::F32 => {
            let callback = make_callback(
                buffer_clone.clone(),
                jvm.clone(),
                target_ref.clone(),
                last_sent.clone(),
                endpoint_cb.clone(),
            );
            device.build_input_stream(
                &input_format.config,
                move |data: &[f32], _| callback(converter.lock().unwrap().convert(data)),
                make_error(),
                None,
            )
        }
        cpal::SampleFormat::I16 => {
            let callback = make_callback(
                buffer_clone.clone(),
                jvm.clone(),
                target_ref.clone(),
                last_sent.clone(),
                endpoint_cb.clone(),
            );
            device.build_input_stream(
                &input_format.config,
                move |data: &[i16], _| {
                    callback(converter.lock().unwrap().convert(&audio::i16_to_f32(data)))
                },
                make_error(),
                None,
            )
        }
        cpal::SampleFormat::U16 => {
            let callback = make_callback(buffer_clone, jvm, target_ref, last_sent, endpoint_cb);
            device.build_input_stream(
                &input_format.config,
                move |data: &[u16], _| {
                    callback(converter.lock().unwrap().convert(&audio::u16_to_f32(data)))
                },
                make_error(),
                None,
            )
        }
        _ => unreachable!(),
    };

    match stream {
        Ok(s) => {
            if s.play().is_err() {
                log::error!("audio recorder play failed");
                state.session_active.store(false, Ordering::SeqCst);
                notify_status(
                    &mut env,
                    state.target_ref.as_obj(),
                    "Error: microphone recorder unavailable.",
                );
                return;
            }
            state.stream = Some(SendStream(s));
            notify_status(&mut env, state.target_ref.as_obj(), "Listening...");

            if let Some(ep) = endpoint {
                let jvm = state.jvm.clone();
                let target_ref = state.target_ref.clone();
                let started_at = Instant::now();
                std::thread::spawn(move || loop {
                    std::thread::sleep(Duration::from_millis(100));
                    if !session_active.load(Ordering::SeqCst) {
                        return;
                    }
                    let speech = ep.speech_started.load(Ordering::SeqCst);
                    let silence = ep.last_voice.lock().unwrap().elapsed();
                    let done = (speech && silence >= Duration::from_millis(AUTO_STOP_SILENCE_MS))
                        || (!speech
                            && started_at.elapsed()
                                >= Duration::from_millis(AUTO_STOP_NO_SPEECH_MS));
                    if done {
                        // Claim the session so a simultaneous manual stop and
                        // this monitor can't both fire.
                        if session_active.swap(false, Ordering::SeqCst) {
                            if let Ok(mut env) = jvm.attach_current_thread() {
                                let _ =
                                    env.call_method(target_ref.as_obj(), "onAutoStop", "()V", &[]);
                            }
                        }
                        return;
                    }
                });
            }
        }
        Err(_) => {
            log::error!("audio recorder open failed");
            state.session_active.store(false, Ordering::SeqCst);
            notify_status(
                &mut env,
                state.target_ref.as_obj(),
                "Error: microphone recorder unavailable.",
            );
        }
    }
}

pub fn stop_recording(mut env: JNIEnv, state: &mut VoiceSessionState) {
    // Drop the stream to stop recording; end the auto-stop monitor if running.
    state.session_active.store(false, Ordering::SeqCst);
    state.stream = None;

    let buffer = state.audio_buffer.lock().unwrap().clone();

    // Guard against empty buffer (mic permission denied, instant stop, etc.)
    if buffer.is_empty() {
        notify_status(
            &mut env,
            state.target_ref.as_obj(),
            "Error: no audio recorded. Check microphone permissions.",
        );
        return;
    }

    let jvm = state.jvm.clone();
    let target_ref = state.target_ref.clone();

    notify_status(&mut env, target_ref.as_obj(), "Transcribing...");

    std::thread::spawn(move || {
        let mut env = match jvm.attach_current_thread() {
            Ok(e) => e,
            Err(_) => return,
        };
        let obj = target_ref.as_obj();

        // Wait for engine if somehow still loading
        if engine::get_engine().is_none() {
            if let Err(_) = engine::ensure_loaded(&mut env, obj) {
                return;
            }
        }

        if let Some(eng_arc) = engine::get_engine() {
            let res = engine::transcribe_shared(&eng_arc, buffer);

            match res {
                Ok(text) => {
                    notify_status(&mut env, obj, "Ready");
                    notify_text(&mut env, obj, &text);
                }
                Err(e) => notify_status(&mut env, obj, &format!("Error: {}", e)),
            }
        } else {
            notify_status(&mut env, obj, "Error: model not loaded");
        }
    });
}

pub fn cancel_recording(mut env: JNIEnv, state: &mut VoiceSessionState) {
    state.session_active.store(false, Ordering::SeqCst);
    state.stream = None;
    state.audio_buffer.lock().unwrap().clear();
    notify_status(&mut env, state.target_ref.as_obj(), "Canceled");
}
