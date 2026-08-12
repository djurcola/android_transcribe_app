use jni::objects::{JClass, JObject};
use jni::JNIEnv;
use once_cell::sync::Lazy;
use std::sync::Mutex;

use crate::engine;
use crate::voice_session::{self, VoiceSessionState};

// This state is intentionally independent from the IME, popup and bubble states.
static BRIDGE_STATE: Lazy<Mutex<Option<VoiceSessionState>>> = Lazy::new(|| Mutex::new(None));

#[no_mangle]
pub unsafe extern "system" fn Java_dev_notune_transcribe_OfflineVoiceBridgeService_initNative(
    env: JNIEnv,
    _class: JClass,
    service: JObject,
) {
    *BRIDGE_STATE.lock().unwrap() = Some(voice_session::init_session(env, service));
}

#[no_mangle]
pub unsafe extern "system" fn Java_dev_notune_transcribe_OfflineVoiceBridgeService_startRecordingNative(
    env: JNIEnv,
    _class: JClass,
) {
    if let Some(state) = BRIDGE_STATE.lock().unwrap().as_mut() {
        voice_session::start_recording(env, state, false);
    }
}

#[no_mangle]
pub unsafe extern "system" fn Java_dev_notune_transcribe_OfflineVoiceBridgeService_stopRecordingNative(
    env: JNIEnv,
    _class: JClass,
) {
    if let Some(state) = BRIDGE_STATE.lock().unwrap().as_mut() {
        voice_session::stop_recording(env, state);
    }
}

#[no_mangle]
pub unsafe extern "system" fn Java_dev_notune_transcribe_OfflineVoiceBridgeService_cancelRecordingNative(
    env: JNIEnv,
    _class: JClass,
) {
    if let Some(state) = BRIDGE_STATE.lock().unwrap().as_mut() {
        voice_session::cancel_recording(env, state);
    }
}

/// Drop the bridge's references, then unload the shared model once any in-flight
/// inference has released it. Other OVI surfaces keep their existing lifecycle.
#[no_mangle]
pub unsafe extern "system" fn Java_dev_notune_transcribe_OfflineVoiceBridgeService_unloadNative(
    _env: JNIEnv,
    _class: JClass,
) {
    *BRIDGE_STATE.lock().unwrap() = None;
    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_millis(100));
        let _ = engine::unload_if_idle();
    });
}
