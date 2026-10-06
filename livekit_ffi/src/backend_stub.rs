//! Stub backend builds on any platform without pulling LiveKit deps.

use std::os::raw::{c_char, c_int, c_void, c_float};
use std::ffi::CString;
use std::sync::{Arc, Mutex};

#[repr(C)]
pub struct LkResult { pub code: c_int, pub message: *const c_char }
fn ok() -> LkResult { LkResult { code: 0, message: std::ptr::null() } }
fn err(msg: &str, code: i32) -> LkResult {
    let c = CString::new(msg).unwrap_or_else(|_| CString::new("ffi error").unwrap());
    LkResult { code, message: c.into_raw() }
}

/// # Safety
/// The caller must ensure that `p` is either NULL or a valid pointer
/// previously allocated by this FFI layer via CString::into_raw.
#[no_mangle]
pub unsafe extern "C" fn lk_free_str(p: *mut c_char) {
    if !p.is_null() {
        let _ = CString::from_raw(p);
    }
}

#[repr(C)] pub enum LkReliability { Reliable = 0, Lossy = 1 }
#[repr(C)] pub enum LkRole { Auto = 0, Publisher = 1, Subscriber = 2, Both = 3 }
#[repr(C)] pub enum LkConnectionState { Connecting = 0, Connected = 1, Reconnecting = 2, Disconnected = 3, Failed = 4 }
#[repr(C)] pub enum LkParticipantEvent { Joined = 0, Left = 1 }
#[repr(C)] pub enum LkLogLevel { Error = 0, Warn = 1, Info = 2, Debug = 3, Trace = 4 }
#[repr(C)] pub struct LkClientHandle { _private: [u8;0] }

#[repr(C)]
pub struct LkAudioStats {
    pub sample_rate: c_int,
    pub channels: c_int,
    pub ring_capacity_frames: c_int,
    pub ring_queued_frames: c_int,
    pub underruns: c_int,
    pub overruns: c_int,
}

#[repr(C)]
pub struct LkDataStats {
    pub reliable_sent_bytes: i64,
    pub reliable_dropped: i64,
    pub lossy_sent_bytes: i64,
    pub lossy_dropped: i64,
}

#[repr(C)]
pub struct LkAudioTrackConfig {
    pub track_name: *const c_char,
    pub sample_rate: c_int,
    pub channels: c_int,
    pub buffer_ms: c_int,
}

#[repr(C)]
pub struct LkAudioTrackHandle {
    _private: [u8; 0],
}

struct ClientState { connected: bool }
struct Client(std::sync::Arc<std::sync::Mutex<ClientState>>);

#[no_mangle] pub extern "C" fn lk_client_create() -> *mut LkClientHandle {
    let state = ClientState { connected: false };
    let boxed = Box::new(Client(Arc::new(Mutex::new(state))));
    Box::into_raw(boxed) as *mut LkClientHandle
}

#[no_mangle] pub extern "C" fn lk_client_destroy(client: *mut LkClientHandle) {
    if client.is_null() { return; }
    unsafe { drop(Box::from_raw(client as *mut Client)); }
}

#[no_mangle] pub extern "C" fn lk_client_set_data_callback(
    _client: *mut LkClientHandle,
    _cb: Option<extern "C" fn(user:*mut c_void, bytes:*const u8, len:usize)>,
    _user: *mut c_void
) -> LkResult { ok() }

#[no_mangle] pub extern "C" fn lk_client_set_data_callback_ex(
    _client: *mut LkClientHandle,
    _cb: Option<extern "C" fn(user:*mut c_void, label:*const c_char, reliability: LkReliability, bytes:*const u8, len:usize)>,
    _user: *mut c_void
) -> LkResult { ok() }

#[no_mangle] pub extern "C" fn lk_client_set_audio_callback(
    _client: *mut LkClientHandle,
    _cb: Option<extern "C" fn(user:*mut c_void, pcm:*const i16, frames_per_channel:usize, channels:c_int, sample_rate:c_int)>,
    _user: *mut c_void
) -> LkResult { ok() }

#[no_mangle] pub extern "C" fn lk_client_set_audio_callback_ex(
    _client: *mut LkClientHandle,
    _cb: Option<extern "C" fn(user:*mut c_void, pcm:*const i16, frames_per_channel:usize, channels:c_int, sample_rate:c_int, participant_name:*const c_char, track_name:*const c_char)>,
    _user: *mut c_void
) -> LkResult { ok() }

#[no_mangle] pub extern "C" fn lk_set_audio_format_change_callback(
    _client: *mut LkClientHandle,
    _cb: Option<extern "C" fn(user:*mut c_void, sample_rate:c_int, channels:c_int)>,
    _user: *mut c_void
) -> LkResult { ok() }

#[no_mangle] pub extern "C" fn lk_set_connection_callback(
    _client: *mut LkClientHandle,
    _cb: Option<extern "C" fn(user:*mut c_void, state:LkConnectionState, reason_code:c_int, message:*const c_char)>,
    _user: *mut c_void
) -> LkResult { ok() }

#[no_mangle] pub extern "C" fn lk_connect(client:*mut LkClientHandle, _url:*const c_char, _token:*const c_char) -> LkResult {
    if client.is_null() { return err("client null", 1); }
    let c = unsafe { &*(client as *const Client) };
    let mut g = c.0.lock().unwrap();
    g.connected = true;
    ok()
}

#[no_mangle] pub extern "C" fn lk_connect_with_role(
    client:*mut LkClientHandle,
    _url:*const c_char,
    _token:*const c_char,
    _role: LkRole
) -> LkResult {
    if client.is_null() { return err("client null", 1); }
    let c = unsafe { &*(client as *const Client) };
    let mut g = c.0.lock().unwrap();
    g.connected = true;
    ok()
}

#[no_mangle] pub extern "C" fn lk_disconnect(client:*mut LkClientHandle) -> LkResult {
    if client.is_null() { return err("client null", 1); }
    let c = unsafe { &*(client as *const Client) };
    let mut g = c.0.lock().unwrap();
    g.connected = false;
    ok()
}

#[no_mangle] pub extern "C" fn lk_client_is_ready(client:*mut LkClientHandle) -> c_int {
    if client.is_null() { return 0; }
    let c = unsafe { &*(client as *const Client) };
    let g = c.0.lock().unwrap();
    if g.connected { 1 } else { 0 }
}

#[no_mangle] pub extern "C" fn lk_set_audio_publish_options(
    _client:*mut LkClientHandle,
    _bitrate_bps:c_int,
    _enable_dtx:c_int,
    _stereo:c_int
) -> LkResult { ok() }

#[no_mangle] pub extern "C" fn lk_set_audio_output_format(
    _client:*mut LkClientHandle,
    _sample_rate:c_int,
    _channels:c_int
) -> LkResult { ok() }

#[no_mangle] pub extern "C" fn lk_publish_audio_pcm_i16(
    client:*mut LkClientHandle,
    _pcm:*const i16,
    _frames_per_ch: usize,
    channels:c_int,
    sample_rate:c_int
) -> LkResult {
    if client.is_null() { return err("client null", 1); }
    if channels <= 0 || sample_rate <= 0 { return err("bad params", 3); }
    ok()
}

#[no_mangle]
pub extern "C" fn lk_audio_track_create(
    client: *mut LkClientHandle,
    _config: *const LkAudioTrackConfig,
    out_track: *mut *mut LkAudioTrackHandle,
) -> LkResult {
    if client.is_null() { return err("client null", 1); }
    if out_track.is_null() { return err("out_track null", 1); }
    unsafe {
        *out_track = Box::into_raw(Box::new(LkAudioTrackHandle { _private: [] }));
    }
    ok()
}

#[no_mangle]
pub extern "C" fn lk_audio_track_destroy(track: *mut LkAudioTrackHandle) -> LkResult {
    if track.is_null() {
        return err("track null", 1);
    }
    unsafe {
        drop(Box::from_raw(track));
    }
    ok()
}

#[no_mangle]
pub extern "C" fn lk_audio_track_publish_pcm_i16(
    track: *mut LkAudioTrackHandle,
    _pcm: *const i16,
    _frames_per_channel: usize,
) -> LkResult {
    if track.is_null() {
        return err("track null", 1);
    }
    ok()
}

#[no_mangle] pub extern "C" fn lk_send_data(
    client:*mut LkClientHandle,
    _bytes:*const u8,
    _len: usize,
    _rel: LkReliability
) -> LkResult {
    if client.is_null() { return err("client null", 1); }
    ok()
}

#[no_mangle] pub extern "C" fn lk_send_data_ex(
    client:*mut LkClientHandle,
    _bytes:*const u8,
    _len: usize,
    _rel: LkReliability,
    _ordered: c_int,
    _label: *const c_char
) -> LkResult {
    if client.is_null() { return err("client null", 1); }
    ok()
}

#[no_mangle] pub extern "C" fn lk_set_default_data_labels(
    _client:*mut LkClientHandle,
    _reliable_label: *const c_char,
    _lossy_label: *const c_char
) -> LkResult { ok() }

#[no_mangle] pub extern "C" fn lk_set_participant_callback(
    _client:*mut LkClientHandle,
    _cb: Option<extern "C" fn(user:*mut c_void, event:LkParticipantEvent, identity:*const c_char, name:*const c_char)>,
    _user: *mut c_void
) -> LkResult { ok() }

#[no_mangle] pub extern "C" fn lk_set_lossy_unreliable(
    _client:*mut LkClientHandle,
    _enable: c_int
) -> LkResult { ok() }

#[no_mangle] pub extern "C" fn lk_set_send_timeout_ms(
    _client:*mut LkClientHandle,
    _timeout_ms: c_int
) -> LkResult { ok() }

#[no_mangle] pub extern "C" fn lk_set_reconnect_backoff(
    _client:*mut LkClientHandle,
    _initial_ms: c_int,
    _max_ms: c_int,
    _multiplier: c_float
) -> LkResult { ok() }

#[no_mangle] pub extern "C" fn lk_refresh_token(
    _client:*mut LkClientHandle,
    _token: *const c_char
) -> LkResult { err("Token refresh not supported in stub backend", 501) }

#[no_mangle] pub extern "C" fn lk_set_role(
    _client:*mut LkClientHandle,
    _role: LkRole,
    _auto_subscribe: c_int
) -> LkResult { err("Dynamic role switching not supported in stub backend", 501) }

#[no_mangle] pub extern "C" fn lk_set_log_level(
    _client:*mut LkClientHandle,
    _level: LkLogLevel
) -> LkResult { ok() }

/// # Safety
/// The caller must ensure `out_stats` points to valid writable memory.
#[no_mangle] pub unsafe extern "C" fn lk_get_audio_stats(
    client:*mut LkClientHandle,
    out_stats: *mut LkAudioStats
) -> LkResult {
    if client.is_null() { return err("client null", 1); }
    if out_stats.is_null() { return err("out_stats null", 1); }
    *out_stats = LkAudioStats {
        sample_rate: 0,
        channels: 0,
        ring_capacity_frames: 0,
        ring_queued_frames: 0,
        underruns: 0,
        overruns: 0,
    };
    ok()
}

/// # Safety
/// The caller must ensure `out_stats` points to valid writable memory.
#[no_mangle] pub unsafe extern "C" fn lk_get_data_stats(
    client:*mut LkClientHandle,
    out_stats: *mut LkDataStats
) -> LkResult {
    if client.is_null() { return err("client null", 1); }
    if out_stats.is_null() { return err("out_stats null", 1); }
    *out_stats = LkDataStats {
        reliable_sent_bytes: 0,
        reliable_dropped: 0,
        lossy_sent_bytes: 0,
        lossy_dropped: 0,
    };
    ok()
}
