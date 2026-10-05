//! LiveKit backend with lock-free SPSC ring buffer for audio frames.
//! Producer: FFI call (UE thread) → push PCM i16 into ring (non-blocking).
//! Consumer: Tokio task → every 10ms pops N samples and feeds NativeAudioSource.
//! Underruns are zero-padded; overflow drops tail to avoid stalling UE audio.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_void, c_float};
use std::ptr;
use std::sync::atomic::{AtomicI32, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use once_cell::sync::OnceCell;
use rtrb::{Producer, RingBuffer};
use tokio::{
    runtime::Runtime,
    task::JoinHandle,
    time::{interval, Duration},
};
use futures::StreamExt;

use livekit::options::TrackPublishOptions;
use livekit::prelude::*;
use livekit::RoomOptions;
use livekit::{ByteStreamWriter, StreamByteOptions, StreamWriter};
use livekit::{DataPacket, DataPacketKind, RoomEvent};
// use livekit::data_stream::ByteStreamReader; // not currently used
use livekit::StreamReader;
use livekit::webrtc::audio_source::{native::NativeAudioSource, AudioSourceOptions, RtcAudioSource};
use livekit::webrtc::prelude::AudioFrame;
use livekit::webrtc::audio_stream::native::NativeAudioStream;

// --------- Internal logging helpers (gated by LkLogLevel) ---------
// A message is emitted if msg_level <= current level. Default level is Error (quiet).
macro_rules! lk_log {
    ($state:expr, $level:expr, $($arg:tt)*) => {{
        if ($level as i32) <= ($state.log_level as i32) {
            println!("[livekit_ffi] {}", format_args!($($arg)*));
        }
    }};
}
macro_rules! lk_log_arc {
    ($arc:expr, $level:expr, $($arg:tt)*) => {{
        if let Ok(__g) = $arc.lock() {
            if ($level as i32) <= (__g.log_level as i32) {
                println!("[livekit_ffi] {}", format_args!($($arg)*));
            }
        }
    }};
}

// --------- C ABI surface ---------

#[repr(C)]
pub struct LkResult {
    pub code: c_int,
    pub message: *const c_char,
}

fn ok() -> LkResult {
    LkResult {
        code: 0,
        message: ptr::null(),
    }
}
fn err(code: i32, msg: &str) -> LkResult {
    let c = CString::new(msg).unwrap_or_else(|_| CString::new("ffi error").unwrap());
    LkResult {
        code,
        message: c.into_raw(),
    }
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

#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub enum LkReliability {
    Reliable = 0,
    Lossy = 1,
}

#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub enum LkRole {
    Auto = 0,
    Publisher = 1,
    Subscriber = 2,
    Both = 3,
}

#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub enum LkConnectionState {
    Connecting = 0,
    Connected = 1,
    Reconnecting = 2,
    Disconnected = 3,
    Failed = 4,
}

#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub enum LkParticipantEvent {
    Joined = 0,
    Left = 1,
}

#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub enum LkLogLevel {
    Error = 0,
    Warn = 1,
    Info = 2,
    Debug = 3,
    Trace = 4,
}

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

struct AudioTrackHandleRef {
    client: Arc<Mutex<ClientState>>,
    track_id: u64,
}

#[repr(C)]
pub struct LkAudioTrackHandle(AudioTrackHandleRef);

#[repr(C)]
pub struct LkClientHandle {
    _private: [u8; 0],
}

// --------- Internal state ---------

struct AudioRing {
    prod: Producer<i16>,
    capacity_frames: usize,
    underruns: Arc<AtomicI32>,
    overruns: Arc<AtomicI32>,
}

impl AudioRing {
    fn queued_frames(&self, channels: u32) -> usize {
        if channels == 0 {
            return 0;
        }
        let total_samples = self.capacity_frames.saturating_mul(channels as usize);
        let free_slots = self.prod.slots();
        let used_samples = total_samples.saturating_sub(free_slots);
        used_samples / channels as usize
    }
}

#[allow(dead_code)]
struct AudioPipeline {
    label: String,
    sample_rate: u32,
    channels: u32,
    ring: AudioRing,
    local_track: LocalAudioTrack,
    src: NativeAudioSource,
    worker: JoinHandle<()>,
}

impl Drop for AudioPipeline {
    fn drop(&mut self) {
        self.worker.abort();
    }
}

impl AudioPipeline {
    fn push(&mut self, data: &[i16]) -> Result<()> {
        if data.len() % self.channels as usize != 0 {
            anyhow::bail!(
                "pcm payload len {} is not divisible by channel count {}",
                data.len(),
                self.channels
            );
        }
        let mut pushed = 0usize;
        let mut dropped = false;
        while pushed < data.len() {
            match self.ring.prod.push(data[pushed]) {
                Ok(_) => pushed += 1,
                Err(_) => {
                    dropped = true;
                    break;
                }
            }
        }
        if dropped {
            self.ring.overruns.fetch_add(1, Ordering::Relaxed);
        }
        Ok(())
    }
}

struct UserPtr(*mut c_void);
unsafe impl Send for UserPtr {}
unsafe impl Sync for UserPtr {}

#[allow(dead_code)]
#[derive(Clone)]
struct AudioPublishOptions {
    bitrate_bps: i32,
    enable_dtx: bool,
    stereo: bool,
}
impl Default for AudioPublishOptions {
    fn default() -> Self {
        Self {
            bitrate_bps: 32_000,
            enable_dtx: false,
            stereo: false,
        }
    }
}

#[derive(Clone)]
struct AudioOutputFormat {
    sample_rate: i32,
    channels: i32,
}

impl Default for AudioOutputFormat {
    fn default() -> Self {
        Self {
            sample_rate: 48_000,
            channels: 1,
        }
    }
}

#[derive(Clone)]
struct DataLabels {
    reliable: String,
    lossy: String,
}

impl Default for DataLabels {
    fn default() -> Self {
        Self {
            reliable: "mocap-bin-reliable".to_string(),
            lossy: "mocap-bin-lossy".to_string(),
        }
    }
}

struct DataStatsCounters {
    reliable_sent_bytes: AtomicI64,
    reliable_dropped: AtomicI64,
    lossy_sent_bytes: AtomicI64,
    lossy_dropped: AtomicI64,
}

impl Default for DataStatsCounters {
    fn default() -> Self {
        Self {
            reliable_sent_bytes: AtomicI64::new(0),
            reliable_dropped: AtomicI64::new(0),
            lossy_sent_bytes: AtomicI64::new(0),
            lossy_dropped: AtomicI64::new(0),
        }
    }
}

/// Fences user callbacks off from lk_disconnect / lk_client_destroy.
///
/// Every event loop and audio task records the generation it was started under and
/// delivers callbacks through `run`. `retire` bumps the generation and then waits for
/// any callback in flight, so once it returns no callback from an earlier session can
/// start. Callbacks run without the client lock held, which lets a callback that
/// queues work to another thread do so without deadlocking against FFI calls.
struct CallbackGate {
    generation: AtomicU64,
    dispatch: Mutex<()>,
}

impl CallbackGate {
    fn new() -> Self {
        Self { generation: AtomicU64::new(0), dispatch: Mutex::new(()) }
    }

    fn current(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    fn run<F: FnOnce()>(&self, generation: u64, f: F) {
        let _d = self.dispatch.lock().unwrap_or_else(|e| e.into_inner());
        if self.generation.load(Ordering::SeqCst) == generation {
            f();
        }
    }

    fn retire(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        drop(self.dispatch.lock().unwrap_or_else(|e| e.into_inner()));
    }
}

/// Copies a callback out of the client state so it can be invoked after the lock is released.
fn cb_copy<F: Copy>(slot: &Option<(F, UserPtr)>) -> Option<(F, *mut c_void)> {
    slot.as_ref().map(|(f, u)| (*f, u.0))
}

/// Upper bound for reading one incoming byte stream. The SDK never closes a stream whose
/// trailer is lost (e.g. the sender's session was replaced mid-stream), and streams are
/// read in order on the event loop, so without a bound one lost trailer would stall every
/// later event.
const STREAM_READ_TIMEOUT: Duration = Duration::from_secs(2);

/// Default bound for lk_send_data / lk_send_data_ex (see lk_set_send_timeout_ms).
const DEFAULT_SEND_TIMEOUT_MS: u64 = 1_000;

struct ClientState {
    room: Option<Room>,
    audio_tracks: HashMap<u64, AudioPipeline>,
    default_audio_track_id: Option<u64>,
    next_audio_track_id: u64,
    rt: Arc<Runtime>,
    
    // Callbacks
    data_cb: Option<(extern "C" fn(*mut c_void, *const u8, usize), UserPtr)>,
    data_cb_ex: Option<(extern "C" fn(*mut c_void, *const c_char, LkReliability, *const u8, usize), UserPtr)>,
    audio_cb: Option<(extern "C" fn(*mut c_void, *const i16, usize, c_int, c_int), UserPtr)>,
    audio_cb_ex: Option<(extern "C" fn(*mut c_void, *const i16, usize, c_int, c_int, *const c_char, *const c_char), UserPtr)>,
    audio_format_change_cb: Option<(extern "C" fn(*mut c_void, c_int, c_int), UserPtr)>,
    connection_cb: Option<(extern "C" fn(*mut c_void, LkConnectionState, c_int, *const c_char), UserPtr)>,
    participant_cb: Option<(extern "C" fn(*mut c_void, LkParticipantEvent, *const c_char, *const c_char), UserPtr)>,
    gate: Arc<CallbackGate>,
    
    // Configuration
    role: LkRole,
    audio_publish_opts: AudioPublishOptions,
    audio_output_format: AudioOutputFormat,
    data_labels: DataLabels,
    log_level: LkLogLevel,
    lossy_unreliable: bool,
    send_timeout_ms: u64,
    
    // Statistics
    data_stats: Arc<DataStatsCounters>,
}

struct Client(Arc<Mutex<ClientState>>);

static RT: OnceCell<Arc<Runtime>> = OnceCell::new();
fn runtime() -> Arc<Runtime> {
    RT.get_or_init(|| Arc::new(Runtime::new().expect("tokio runtime"))).clone()
}

unsafe fn cstr<'a>(p: *const c_char) -> Result<&'a str> {
    if p.is_null() {
        anyhow::bail!("null pointer")
    }
    Ok(CStr::from_ptr(p).to_str()?)
}

// --------- FFI functions ---------

#[no_mangle]
pub extern "C" fn lk_client_create() -> *mut LkClientHandle {
    let state = ClientState {
        room: None,
        audio_tracks: HashMap::new(),
        default_audio_track_id: None,
        next_audio_track_id: 1,
        rt: runtime(),
        data_cb: None,
        data_cb_ex: None,
        audio_cb: None,
        audio_cb_ex: None,
        audio_format_change_cb: None,
        connection_cb: None,
        participant_cb: None,
        gate: Arc::new(CallbackGate::new()),
        role: LkRole::Both,
        audio_publish_opts: AudioPublishOptions::default(),
        audio_output_format: AudioOutputFormat::default(),
        data_labels: DataLabels::default(),
        log_level: LkLogLevel::Error,
        lossy_unreliable: false,
        send_timeout_ms: DEFAULT_SEND_TIMEOUT_MS,
        data_stats: Arc::new(DataStatsCounters::default()),
    };
    let boxed = Box::new(Client(Arc::new(Mutex::new(state))));
    Box::into_raw(boxed) as *mut LkClientHandle
}

#[no_mangle]
pub extern "C" fn lk_client_destroy(client: *mut LkClientHandle) {
    if client.is_null() {
        return;
    }
    let boxed = unsafe { Box::from_raw(client as *mut Client) };
    // Event loops and audio tasks hold their own reference to the state, so retire them
    // explicitly: the header promises no callbacks once this returns.
    let gate = match boxed.0.lock() {
        Ok(g) => g.gate.clone(),
        Err(e) => e.into_inner().gate.clone(),
    };
    gate.retire();
    let (room, rt) = match boxed.0.lock() {
        Ok(mut g) => {
            g.audio_tracks.clear();
            (g.room.take(), g.rt.clone())
        }
        Err(_) => (None, runtime()),
    };
    if let Some(room) = room {
        // block_on would panic on a runtime thread; there the room is just dropped.
        if tokio::runtime::Handle::try_current().is_err() {
            rt.block_on(async move {
                let _ = room.close().await;
            });
        }
    }
    drop(boxed);
}

#[no_mangle]
pub extern "C" fn lk_client_set_data_callback(
    client: *mut LkClientHandle,
    cb: Option<extern "C" fn(user: *mut c_void, bytes: *const u8, len: usize)>,
    user: *mut c_void,
) -> LkResult {
    if client.is_null() { return err(1, "client null"); }
    let c = unsafe { &*(client as *const Client) };
    let mut g = c.0.lock().unwrap();
    g.data_cb = cb.map(|f| (f, UserPtr(user)));
    ok()
}

#[no_mangle]
pub extern "C" fn lk_client_set_audio_callback(
    client: *mut LkClientHandle,
    cb: Option<extern "C" fn(user: *mut c_void, pcm: *const i16, frames_per_channel: usize, channels: c_int, sample_rate: c_int)>,
    user: *mut c_void,
) -> LkResult {
    if client.is_null() { return err(1, "client null"); }
    let c = unsafe { &*(client as *const Client) };
    let mut g = c.0.lock().unwrap();
    g.audio_cb = cb.map(|f| (f, UserPtr(user)));
    // Clear extended callback if standard callback is set
    if cb.is_some() {
        g.audio_cb_ex = None;
    }
    ok()
}

#[no_mangle]
pub extern "C" fn lk_client_set_audio_callback_ex(
    client: *mut LkClientHandle,
    cb: Option<extern "C" fn(user: *mut c_void, pcm: *const i16, frames_per_channel: usize, channels: c_int, sample_rate: c_int, participant_name: *const c_char, track_name: *const c_char)>,
    user: *mut c_void,
) -> LkResult {
    if client.is_null() { return err(1, "client null"); }
    let c = unsafe { &*(client as *const Client) };
    let mut g = c.0.lock().unwrap();
    g.audio_cb_ex = cb.map(|f| (f, UserPtr(user)));
    // Clear standard callback if extended callback is set
    if cb.is_some() {
        g.audio_cb = None;
    }
    ok()
}

#[no_mangle]
pub extern "C" fn lk_client_set_data_callback_ex(
    client: *mut LkClientHandle,
    cb: Option<extern "C" fn(user: *mut c_void, label: *const c_char, reliability: LkReliability, bytes: *const u8, len: usize)>,
    user: *mut c_void,
) -> LkResult {
    if client.is_null() { return err(1, "client null"); }
    let c = unsafe { &*(client as *const Client) };
    let mut g = c.0.lock().unwrap();
    g.data_cb_ex = cb.map(|f| (f, UserPtr(user)));
    ok()
}

#[no_mangle]
pub extern "C" fn lk_set_audio_format_change_callback(
    client: *mut LkClientHandle,
    cb: Option<extern "C" fn(user: *mut c_void, sample_rate: c_int, channels: c_int)>,
    user: *mut c_void,
) -> LkResult {
    if client.is_null() { return err(1, "client null"); }
    let c = unsafe { &*(client as *const Client) };
    let mut g = c.0.lock().unwrap();
    g.audio_format_change_cb = cb.map(|f| (f, UserPtr(user)));
    ok()
}

#[no_mangle]
pub extern "C" fn lk_set_connection_callback(
    client: *mut LkClientHandle,
    cb: Option<extern "C" fn(user: *mut c_void, state: LkConnectionState, reason_code: c_int, message: *const c_char)>,
    user: *mut c_void,
) -> LkResult {
    if client.is_null() { return err(1, "client null"); }
    let c = unsafe { &*(client as *const Client) };
    let mut g = c.0.lock().unwrap();
    g.connection_cb = cb.map(|f| (f, UserPtr(user)));
    ok()
}

#[no_mangle]
pub extern "C" fn lk_set_participant_callback(
    client: *mut LkClientHandle,
    cb: Option<extern "C" fn(user: *mut c_void, event: LkParticipantEvent, identity: *const c_char, name: *const c_char)>,
    user: *mut c_void,
) -> LkResult {
    if client.is_null() { return err(1, "client null"); }
    let c = unsafe { &*(client as *const Client) };
    let mut g = c.0.lock().unwrap();
    g.participant_cb = cb.map(|f| (f, UserPtr(user)));
    ok()
}

// --------- Configuration Functions ---------

#[no_mangle]
pub extern "C" fn lk_set_lossy_unreliable(
    client: *mut LkClientHandle,
    enable: c_int,
) -> LkResult {
    if client.is_null() { return err(1, "client null"); }
    let c = unsafe { &*(client as *const Client) };
    let mut g = c.0.lock().unwrap();
    g.lossy_unreliable = enable != 0;
    lk_log!(g, LkLogLevel::Debug, "Lossy transport: {}", if g.lossy_unreliable { "unreliable data packets" } else { "reliable byte streams" });
    ok()
}

#[no_mangle]
pub extern "C" fn lk_set_send_timeout_ms(
    client: *mut LkClientHandle,
    timeout_ms: c_int,
) -> LkResult {
    if client.is_null() { return err(1, "client null"); }
    if timeout_ms < 0 { return err(5, "timeout_ms must be >= 0"); }
    let c = unsafe { &*(client as *const Client) };
    let mut g = c.0.lock().unwrap();
    g.send_timeout_ms = timeout_ms as u64;
    lk_log!(g, LkLogLevel::Debug, "Send timeout set to {} ms (0 = unbounded)", timeout_ms);
    ok()
}

#[no_mangle]
pub extern "C" fn lk_set_audio_publish_options(
    client: *mut LkClientHandle,
    bitrate_bps: c_int,
    enable_dtx: c_int,
    stereo: c_int,
) -> LkResult {
    if client.is_null() { return err(1, "client null"); }
    let c = unsafe { &*(client as *const Client) };
    let mut g = c.0.lock().unwrap();
    g.audio_publish_opts = AudioPublishOptions {
        bitrate_bps,
        enable_dtx: enable_dtx != 0,
        stereo: stereo != 0,
    };
    lk_log!(g, LkLogLevel::Debug, "Audio publish options set: bitrate={}bps, dtx={}, stereo={}", bitrate_bps, enable_dtx != 0, stereo != 0);
    ok()
}

#[no_mangle]
pub extern "C" fn lk_set_audio_output_format(
    client: *mut LkClientHandle,
    sample_rate: c_int,
    channels: c_int,
) -> LkResult {
    if client.is_null() { return err(1, "client null"); }
    if sample_rate <= 0 || channels <= 0 {
        return err(5, "invalid audio output format");
    }
    let c = unsafe { &*(client as *const Client) };
    let mut g = c.0.lock().unwrap();
    g.audio_output_format = AudioOutputFormat {
        sample_rate,
        channels,
    };
    lk_log!(g, LkLogLevel::Debug, "Audio output format set: sr={}Hz, ch={}", sample_rate, channels);
    ok()
}

#[no_mangle]
pub extern "C" fn lk_set_default_data_labels(
    client: *mut LkClientHandle,
    reliable_label: *const c_char,
    lossy_label: *const c_char,
) -> LkResult {
    if client.is_null() { return err(1, "client null"); }
    let c = unsafe { &*(client as *const Client) };
    let mut g = c.0.lock().unwrap();
    
    if !reliable_label.is_null() {
        if let Ok(s) = unsafe { cstr(reliable_label) } {
            g.data_labels.reliable = s.to_string();
        }
    }
    if !lossy_label.is_null() {
        if let Ok(s) = unsafe { cstr(lossy_label) } {
            g.data_labels.lossy = s.to_string();
        }
    }
    
    lk_log!(g, LkLogLevel::Debug, "Data labels set: reliable='{}', lossy='{}'", g.data_labels.reliable, g.data_labels.lossy);
    ok()
}

#[no_mangle]
pub extern "C" fn lk_set_reconnect_backoff(
    client: *mut LkClientHandle,
    _initial_ms: c_int,
    _max_ms: c_int,
    _multiplier: c_float,
) -> LkResult {
    // Note: LiveKit SDK manages reconnection internally; this is a placeholder
    // for future implementation if SDK exposes these controls
    if !client.is_null() {
        let c = unsafe { &*(client as *const Client) };
        if let Ok(g) = c.0.lock() {
            lk_log!(g, LkLogLevel::Trace, "Reconnect backoff configuration requested (not yet implemented)");
        }
    }
    ok()
}

#[no_mangle]
pub extern "C" fn lk_refresh_token(
    _client: *mut LkClientHandle,
    _token: *const c_char,
) -> LkResult {
    // Note: Token refresh at runtime is not currently supported by LiveKit SDK
    // Best practice is to disconnect and reconnect with new token
    err(501, "Token refresh not supported; use disconnect + reconnect")
}

#[no_mangle]
pub extern "C" fn lk_set_role(
    _client: *mut LkClientHandle,
    _role: LkRole,
    _auto_subscribe: c_int,
) -> LkResult {
    // Note: Dynamic role switching without reconnect is not currently supported
    // Best practice is to disconnect and reconnect with new role
    err(501, "Dynamic role switching not supported; use disconnect + reconnect with new role")
}

#[no_mangle]
pub extern "C" fn lk_set_log_level(
    client: *mut LkClientHandle,
    level: LkLogLevel,
) -> LkResult {
    if client.is_null() { return err(1, "client null"); }
    let c = unsafe { &*(client as *const Client) };
    let mut g = c.0.lock().unwrap();
    g.log_level = level;
    lk_log!(g, LkLogLevel::Debug, "Log level set to: {:?}", level);
    ok()
}

// --------- Event loop (shared by sync and async connect) ---------

/// Everything a session's event loop and its audio tasks need to deliver callbacks.
/// Callbacks are copied out under the client lock and invoked after it is released,
/// through the gate, so lk_disconnect can retire them.
#[derive(Clone)]
struct EventCtx {
    client: Arc<Mutex<ClientState>>,
    gate: Arc<CallbackGate>,
    generation: u64,
}

impl EventCtx {
    fn new(client: &Arc<Mutex<ClientState>>, gate: Arc<CallbackGate>, generation: u64) -> Self {
        Self { client: client.clone(), gate, generation }
    }

    fn emit_data(&self, label: &str, reliability: LkReliability, bytes: &[u8]) {
        let (ex, plain) = match self.client.lock() {
            Ok(g) => (cb_copy(&g.data_cb_ex), cb_copy(&g.data_cb)),
            Err(_) => return,
        };
        let label_c = CString::new(label).unwrap_or_default();
        self.gate.run(self.generation, || {
            // The extended callback wins when both are set; no packet reaches both.
            if let Some((cb, user)) = ex {
                cb(user, label_c.as_ptr(), reliability, bytes.as_ptr(), bytes.len());
            } else if let Some((cb, user)) = plain {
                cb(user, bytes.as_ptr(), bytes.len());
            }
        });
    }

    fn emit_connection(&self, state: LkConnectionState, reason_code: c_int, message: Option<&str>) {
        let cb = match self.client.lock() {
            Ok(g) => cb_copy(&g.connection_cb),
            Err(_) => return,
        };
        let Some((cb, user)) = cb else { return; };
        let msg = message.map(|m| CString::new(m).unwrap_or_default());
        self.gate.run(self.generation, || {
            cb(user, state, reason_code, msg.as_ref().map_or(ptr::null(), |m| m.as_ptr()));
        });
    }

    fn emit_participant(&self, event: LkParticipantEvent, identity: &str, name: &str) {
        let cb = match self.client.lock() {
            Ok(g) => cb_copy(&g.participant_cb),
            Err(_) => return,
        };
        let Some((cb, user)) = cb else { return; };
        let identity_c = CString::new(identity).unwrap_or_default();
        let name_c = CString::new(name).unwrap_or_default();
        self.gate.run(self.generation, || {
            cb(user, event, identity_c.as_ptr(), name_c.as_ptr());
        });
    }

    fn emit_audio(&self, pcm: &[i16], frames_per_channel: usize, channels: c_int, sample_rate: c_int, participant: &CStr, track: &CStr) {
        let (ex, plain) = match self.client.lock() {
            Ok(g) => (cb_copy(&g.audio_cb_ex), cb_copy(&g.audio_cb)),
            Err(_) => return,
        };
        self.gate.run(self.generation, || {
            if let Some((cb, user)) = ex {
                cb(user, pcm.as_ptr(), frames_per_channel, channels, sample_rate, participant.as_ptr(), track.as_ptr());
            } else if let Some((cb, user)) = plain {
                cb(user, pcm.as_ptr(), frames_per_channel, channels, sample_rate);
            }
        });
    }
}

fn to_lk_state(state: livekit::ConnectionState) -> LkConnectionState {
    match state {
        livekit::ConnectionState::Disconnected => LkConnectionState::Disconnected,
        livekit::ConnectionState::Connected => LkConnectionState::Connected,
        livekit::ConnectionState::Reconnecting => LkConnectionState::Reconnecting,
    }
}

/// Remote participants already in the room. Room::connect creates them from the join
/// response without dispatching ParticipantConnected, so the loop announces them itself.
fn present_participants(room: &Room) -> Vec<(String, String)> {
    room.remote_participants()
        .values()
        .map(|p| (p.identity().to_string(), p.name()))
        .collect()
}

fn spawn_audio_forwarder(ctx: EventCtx, audio: RemoteAudioTrack, participant_identity: String, track_name: String) {
    let (sample_rate, channels) = match ctx.client.lock() {
        Ok(g) => (g.audio_output_format.sample_rate, g.audio_output_format.channels),
        Err(_) => (48_000, 1),
    };
    let rtc = audio.rtc_track();
    tokio::spawn(async move {
        let mut stream = NativeAudioStream::new(rtc, sample_rate, channels);
        let participant_c = CString::new(participant_identity).unwrap_or_default();
        let track_c = CString::new(track_name).unwrap_or_default();
        let mut logged_first = false;
        while let Some(frame) = stream.next().await {
            if ctx.gate.current() != ctx.generation {
                break;
            }
            ctx.emit_audio(
                frame.data.as_ref(),
                frame.samples_per_channel as usize,
                frame.num_channels as c_int,
                frame.sample_rate as c_int,
                &participant_c,
                &track_c,
            );
            if !logged_first {
                lk_log_arc!(ctx.client, LkLogLevel::Debug, "First remote audio frame: sr={}Hz, ch={}, fpc={}", frame.sample_rate, frame.num_channels, frame.samples_per_channel);
                logged_first = true;
            }
        }
    });
}

/// The one event loop both connect paths use, so they cannot drift apart again (#13).
fn spawn_event_loop(
    rt: &Runtime,
    ctx: EventCtx,
    mut events: tokio::sync::mpsc::UnboundedReceiver<RoomEvent>,
    initial_participants: Vec<(String, String)>,
) {
    rt.spawn(async move {
        // Identities announced as joined. Joined/Left fire only on a real change, so a
        // participant who shows up both in the connect-time snapshot and as a queued
        // ParticipantConnected is announced once.
        let mut present: HashSet<String> = HashSet::new();
        for (identity, name) in initial_participants {
            if present.insert(identity.clone()) {
                lk_log_arc!(ctx.client, LkLogLevel::Info, "Participant present at connect: '{}'", identity);
                ctx.emit_participant(LkParticipantEvent::Joined, &identity, &name);
            }
        }

        while let Some(ev) = events.recv().await {
            if ctx.gate.current() != ctx.generation {
                break;
            }
            match ev {
                RoomEvent::ParticipantConnected(participant) => {
                    let identity = participant.identity().to_string();
                    if present.insert(identity.clone()) {
                        lk_log_arc!(ctx.client, LkLogLevel::Info, "Participant joined: '{}'", identity);
                        ctx.emit_participant(LkParticipantEvent::Joined, &identity, &participant.name());
                    }
                }
                RoomEvent::ParticipantDisconnected(participant) => {
                    let identity = participant.identity().to_string();
                    if present.remove(&identity) {
                        lk_log_arc!(ctx.client, LkLogLevel::Info, "Participant left: '{}'", identity);
                        ctx.emit_participant(LkParticipantEvent::Left, &identity, &participant.name());
                    }
                }
                RoomEvent::ByteStreamOpened { reader, topic, participant_identity } => {
                    let Some(reader) = reader.take() else { continue; };
                    // Read inline: callbacks then arrive in the order the streams were opened.
                    match tokio::time::timeout(STREAM_READ_TIMEOUT, reader.read_all()).await {
                        Ok(Ok(content)) => {
                            lk_log_arc!(ctx.client, LkLogLevel::Debug, "ByteStreamOpened: received {} bytes on topic '{}' from '{}'", content.len(), topic, participant_identity);
                            // Byte streams always travel on the reliable, ordered channel.
                            ctx.emit_data(&topic, LkReliability::Reliable, &content);
                        }
                        Ok(Err(e)) => {
                            lk_log_arc!(ctx.client, LkLogLevel::Warn, "Dropped incomplete byte stream on topic '{}' from '{}': {}", topic, participant_identity, e);
                        }
                        Err(_) => {
                            lk_log_arc!(ctx.client, LkLogLevel::Warn, "Dropped byte stream on topic '{}' from '{}': not completed within {:?}", topic, participant_identity, STREAM_READ_TIMEOUT);
                        }
                    }
                }
                RoomEvent::DataReceived { payload, topic, kind, participant: _ } => {
                    let reliability = match kind {
                        DataPacketKind::Lossy => LkReliability::Lossy,
                        DataPacketKind::Reliable => LkReliability::Reliable,
                    };
                    let label = topic.as_deref().unwrap_or("");
                    lk_log_arc!(ctx.client, LkLogLevel::Debug, "DataReceived: {} bytes, topic '{}', {:?}", payload.len(), label, kind);
                    ctx.emit_data(label, reliability, &payload);
                }
                RoomEvent::Disconnected { reason } => {
                    lk_log_arc!(ctx.client, LkLogLevel::Info, "Disconnected event: reason={:?}", reason);
                    ctx.emit_connection(LkConnectionState::Disconnected, 0, Some(&format!("{:?}", reason)));
                }
                RoomEvent::ConnectionStateChanged(state) => {
                    lk_log_arc!(ctx.client, LkLogLevel::Debug, "ConnectionStateChanged: {:?}", state);
                    ctx.emit_connection(to_lk_state(state), 0, None);
                }
                RoomEvent::TrackSubscribed { track, publication, participant } => {
                    if let RemoteTrack::Audio(audio) = track {
                        lk_log_arc!(ctx.client, LkLogLevel::Info, "TrackSubscribed audio: name='{}', sid='{}', participant='{}'", publication.name(), publication.sid(), participant.identity());
                        spawn_audio_forwarder(ctx.clone(), audio, participant.identity().to_string(), publication.name().to_string());
                    }
                }
                other => {
                    lk_log_arc!(ctx.client, LkLogLevel::Trace, "Event: {:?}", other);
                }
            }
        }
    });
}

// --------- Connection Functions ---------

#[no_mangle]
pub extern "C" fn lk_connect(
    client: *mut LkClientHandle,
    url: *const c_char,
    token: *const c_char,
) -> LkResult {
    // Default to Both
    lk_connect_with_role(client, url, token, LkRole::Both)
}

#[no_mangle]
pub extern "C" fn lk_connect_with_role(
    client: *mut LkClientHandle,
    url: *const c_char,
    token: *const c_char,
    role: LkRole,
) -> LkResult {
    if client.is_null() {
        return err(1, "client null");
    }

    let url = unsafe { match cstr(url) {
        Ok(s) => s.to_string(),
        Err(e) => return err(2, &e.to_string()),
    }};
    let token = unsafe { match cstr(token) {
        Ok(s) => s.to_string(),
        Err(e) => return err(2, &e.to_string()),
    }};

    let c = unsafe { &*(client as *const Client) };
    let mut g = c.0.lock().unwrap();
    let rt = g.rt.clone();

    let role_copy = role; // copy enum (Copy)
    let res = rt.block_on(async move {
        let mut opts = RoomOptions::default();
        // If explicit Publisher, disable auto_subscribe to avoid subscribing to media.
        if matches!(role_copy, LkRole::Publisher) { opts.auto_subscribe = false; }
        let (room, events) = Room::connect(&url, &token, opts).await?;
        Ok::<(Room, tokio::sync::mpsc::UnboundedReceiver<RoomEvent>), anyhow::Error>((room, events))
    });

    match res {
        Ok((room, events)) => {
            g.role = role_copy;
            lk_log!(g, LkLogLevel::Info, "Connected. role={:?} auto_subscribe={}", role_copy, !matches!(role_copy, LkRole::Publisher));

            // Notify connection established
            if let Some((cb, user)) = g.connection_cb.as_ref() {
                cb(user.0, LkConnectionState::Connected, 0, ptr::null());
            }

            // The loop starts with the participants already present; it blocks on the
            // client lock until this call returns, so they are reported after Connected.
            let ctx = EventCtx::new(&c.0, g.gate.clone(), g.gate.current());
            spawn_event_loop(&rt, ctx, events, present_participants(&room));
            g.room = Some(room);
            ok()
        }
        Err(e) => err(3, &format!("connect failed: {e}")),
    }
}

#[no_mangle]
pub extern "C" fn lk_connect_async(
    client: *mut LkClientHandle,
    url: *const c_char,
    token: *const c_char,
) -> LkResult {
    // Default to Both
    lk_connect_with_role_async(client, url, token, LkRole::Both)
}

#[no_mangle]
pub extern "C" fn lk_connect_with_role_async(
    client: *mut LkClientHandle,
    url: *const c_char,
    token: *const c_char,
    role: LkRole,
) -> LkResult {
    if client.is_null() {
        return err(1, "client null");
    }

    let url = unsafe { match cstr(url) {
        Ok(s) => s.to_string(),
        Err(e) => return err(2, &e.to_string()),
    }};
    let token = unsafe { match cstr(token) {
        Ok(s) => s.to_string(),
        Err(e) => return err(2, &e.to_string()),
    }};

    let c = unsafe { &*(client as *const Client) };
    let client_arc = c.0.clone();

    // Early-out if already connected
    let ctx = {
        let g = match client_arc.lock() {
            Ok(g) => g,
            Err(_) => return err(5, "client state poisoned"),
        };
        if g.room.is_some() {
            return err(104, "already connected");
        }
        // Notify connecting state if callback present
        if let Some((cb, user)) = g.connection_cb.as_ref() {
            cb(user.0, LkConnectionState::Connecting, 0, ptr::null());
        }
        // A disconnect issued before the attempt completes retires this generation.
        EventCtx::new(&client_arc, g.gate.clone(), g.gate.current())
    };

    // Spawn the connection attempt without blocking the caller
    let rt = runtime();
    let rt_loop = rt.clone();
    rt.spawn(async move {
        let mut opts = RoomOptions::default();
        if matches!(role, LkRole::Publisher) { opts.auto_subscribe = false; }
        match Room::connect(&url, &token, opts).await {
            Ok((room, events)) => {
                let initial = present_participants(&room);
                // Install the room unless lk_disconnect ran (or another connect won) meanwhile.
                let stale_room = match ctx.client.lock() {
                    Ok(mut g) if g.gate.current() == ctx.generation && g.room.is_none() => {
                        g.role = role;
                        g.room = Some(room);
                        lk_log!(g, LkLogLevel::Info, "Connected (async). role={:?} auto_subscribe={}", role, !matches!(role, LkRole::Publisher));
                        None
                    }
                    _ => Some(room),
                };
                if let Some(room) = stale_room {
                    lk_log_arc!(ctx.client, LkLogLevel::Info, "Async connect completed after disconnect; closing the new session");
                    let _ = room.close().await;
                    return;
                }
                ctx.emit_connection(LkConnectionState::Connected, 0, None);
                spawn_event_loop(&rt_loop, ctx, events, initial);
            }
            Err(e) => {
                lk_log_arc!(ctx.client, LkLogLevel::Error, "Async connect failed: {}", e);
                ctx.emit_connection(LkConnectionState::Failed, 1, Some(&e.to_string()));
            }
        }
    });

    ok()
}

#[no_mangle]
pub extern "C" fn lk_disconnect(client: *mut LkClientHandle) -> LkResult {
    if client.is_null() {
        return err(1, "client null");
    }
    if tokio::runtime::Handle::try_current().is_ok() {
        return err(403, "lk_disconnect cannot be called from an FFI callback; call it from your own thread");
    }
    let c = unsafe { &*(client as *const Client) };

    // Retire this session's callbacks before taking the client lock: a callback in flight
    // may itself be waiting on that lock (e.g. a send from a participant callback).
    let gate = c.0.lock().unwrap().gate.clone();
    gate.retire();

    let mut g = c.0.lock().unwrap();
    let had_room = if let Some(room) = g.room.take() {
        let rt = g.rt.clone();
        let _ = rt.block_on(async move {
            let _ = room.close().await; // graceful shutdown
        });
        true
    } else {
        false
    };
    lk_log!(g, LkLogLevel::Info, "Disconnected");
    g.audio_tracks.clear();
    g.default_audio_track_id = None;
    let conn_cb = if had_room { cb_copy(&g.connection_cb) } else { None };
    drop(g);

    // The room's own Disconnected event belongs to the retired generation, so report the
    // disconnect here, on the caller's thread, before returning.
    if let Some((cb, user)) = conn_cb {
        let msg = CString::new("ClientInitiated").unwrap_or_default();
        cb(user, LkConnectionState::Disconnected, 0, msg.as_ptr());
    }
    ok()
}

#[no_mangle]
pub extern "C" fn lk_client_is_ready(client: *mut LkClientHandle) -> c_int {
    if client.is_null() {
        return 0;
    }
    let c = unsafe { &*(client as *const Client) };
    let g = c.0.lock().unwrap();
    if g.room.is_some() { 1 } else { 0 }
}

fn next_audio_track_id(g: &mut ClientState) -> u64 {
    let id = g.next_audio_track_id;
    g.next_audio_track_id = g.next_audio_track_id.wrapping_add(1);
    if g.next_audio_track_id == 0 {
        g.next_audio_track_id = 1;
    }
    id
}

fn register_audio_pipeline(
    g: &mut ClientState,
    label: &str,
    sample_rate: u32,
    channels: u32,
    buffer_ms: u32,
) -> Result<u64> {
    let id = next_audio_track_id(g);
    let pipeline = create_audio_pipeline(g, label, sample_rate, channels, buffer_ms)?;
    g.audio_tracks.insert(id, pipeline);
    Ok(id)
}

fn ensure_default_audio_track(g: &mut ClientState, sample_rate: u32, channels: u32) -> Result<u64> {
    if let Some(id) = g.default_audio_track_id {
        if let Some(pipeline) = g.audio_tracks.get(&id) {
            if pipeline.sample_rate != sample_rate || pipeline.channels != channels {
                anyhow::bail!(
                    "default audio track already configured for {} Hz ({} ch), requested {} Hz ({} ch)",
                    pipeline.sample_rate,
                    pipeline.channels,
                    sample_rate,
                    channels
                );
            }
            return Ok(id);
        }
        g.default_audio_track_id = None;
    }
    let id = register_audio_pipeline(g, "ue-audio", sample_rate, channels, 1_000)?;
    g.default_audio_track_id = Some(id);
    Ok(id)
}

fn create_audio_pipeline(
    g: &mut ClientState,
    label: &str,
    sample_rate: u32,
    channels: u32,
    buffer_ms: u32,
) -> Result<AudioPipeline> {
    if sample_rate == 0 || channels == 0 {
        anyhow::bail!("invalid audio parameters");
    }
    let buffer_ms = buffer_ms.clamp(100, 5_000);
    let samples_per_10ms = (sample_rate / 100).max(1);
    let src = NativeAudioSource::new(
        AudioSourceOptions::default(),
        sample_rate,
        channels,
        samples_per_10ms,
    );
    let local = LocalAudioTrack::create_audio_track(label, RtcAudioSource::Native(src.clone()));
    let room = g
        .room
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("not connected"))?;
    let rt = g.rt.clone();
    let publish_res = rt.block_on(async {
        room.local_participant()
            .publish_track(LocalTrack::Audio(local.clone()), TrackPublishOptions::default())
            .await
    });
    match publish_res {
        Ok(_) => lk_log!(
            g,
            LkLogLevel::Info,
            "Published audio track '{}' (sr={} ch={} buffer={}ms)",
            label,
            sample_rate,
            channels,
            buffer_ms
        ),
        Err(e) => {
            lk_log!(
                g,
                LkLogLevel::Error,
                "Failed to publish audio track '{}': {}",
                label,
                e
            );
            return Err(e.into());
        }
    }

    let safe_channels = channels.max(1);
    let safe_channels_for_worker = safe_channels;
    let capacity_samples = ((sample_rate as usize * channels as usize) * buffer_ms as usize / 1_000)
        .max(samples_per_10ms as usize * channels as usize)
        .max(1);
    let (prod, mut cons) = RingBuffer::<i16>::new(capacity_samples);
    let frame_samples = ((sample_rate as usize / 100) * channels as usize).max(1);
    let underruns = Arc::new(AtomicI32::new(0));
    let overruns = Arc::new(AtomicI32::new(0));
    let underruns_clone = underruns.clone();
    let src_clone = src.clone();
    let consumer_rt = g.rt.clone();

    let worker = consumer_rt.spawn(async move {
        let mut tick = interval(Duration::from_millis(10));
        let mut buf: Vec<i16> = vec![0; frame_samples];
        loop {
            tick.tick().await;

            let mut got = 0usize;
            while got < buf.len() {
                match cons.pop() {
                    Ok(s) => {
                        buf[got] = s;
                        got += 1;
                    }
                    Err(_) => break,
                }
            }
            if got < buf.len() {
                underruns_clone.fetch_add(1, Ordering::Relaxed);
                for x in &mut buf[got..] {
                    *x = 0;
                }
            }

            let samples_per_channel = (buf.len() as u32) / safe_channels_for_worker;
            let frame = AudioFrame {
                data: Cow::Borrowed(&buf[..]),
                sample_rate,
                num_channels: channels,
                samples_per_channel,
            };
            let _ = src_clone.capture_frame(&frame).await;
        }
    });

    let ring = AudioRing {
        prod,
        capacity_frames: (capacity_samples / (safe_channels as usize)).max(1),
        underruns,
        overruns,
    };

    Ok(AudioPipeline {
        label: label.to_string(),
        sample_rate,
        channels,
        ring,
        local_track: local,
        src,
        worker,
    })
}

#[no_mangle]
pub extern "C" fn lk_publish_audio_pcm_i16(
    client: *mut LkClientHandle,
    pcm: *const i16,
    frames_per_channel: usize,
    channels: c_int,
    sample_rate: c_int,
) -> LkResult {
    if client.is_null() {
        return err(1, "client null");
    }
    if pcm.is_null() {
        return err(4, "pcm null");
    }
    if channels <= 0 || sample_rate <= 0 {
        return err(5, "bad params");
    }

    let c = unsafe { &*(client as *const Client) };
    let mut g = c.0.lock().unwrap();
    if g.room.is_none() {
        return err(6, "not connected");
    }

    let channels = channels as u32;
    let sample_rate = sample_rate as u32;

    let track_id = match ensure_default_audio_track(&mut g, sample_rate, channels) {
        Ok(id) => id,
        Err(e) => {
            let msg = format!("audio pipeline init failed: {}", e);
            lk_log!(g, LkLogLevel::Error, "{}", msg);
            return err(7, &msg);
        }
    };

    let total = frames_per_channel * channels as usize;
    let slice = unsafe { std::slice::from_raw_parts(pcm, total) };

    match g.audio_tracks.get_mut(&track_id) {
        Some(pipeline) => {
            if let Err(e) = pipeline.push(slice) {
                let msg = format!("audio ring push failed: {}", e);
                lk_log!(g, LkLogLevel::Error, "{}", msg);
                return err(8, &msg);
            }
        }
        None => {
            let msg = "audio pipeline disappeared";
            lk_log!(g, LkLogLevel::Error, "{}", msg);
            return err(8, msg);
        }
    }

    ok()
}

#[no_mangle]
pub extern "C" fn lk_audio_track_create(
    client: *mut LkClientHandle,
    config: *const LkAudioTrackConfig,
    out_track: *mut *mut LkAudioTrackHandle,
) -> LkResult {
    if client.is_null() {
        return err(1, "client null");
    }
    if config.is_null() {
        return err(5, "config null");
    }
    if out_track.is_null() {
        return err(5, "out_track null");
    }

    let cfg = unsafe { &*config };
    if cfg.sample_rate <= 0 || cfg.channels <= 0 {
        return err(5, "invalid audio track parameters");
    }

    let label = if cfg.track_name.is_null() {
        "ue-audio-track"
    } else {
        match unsafe { cstr(cfg.track_name) } {
            Ok(s) => s,
            Err(_) => "ue-audio-track",
        }
    };
    let buffer_ms = if cfg.buffer_ms <= 0 { 1_000 } else { cfg.buffer_ms };

    let c = unsafe { &*(client as *const Client) };
    let mut g = c.0.lock().unwrap();
    let track_id = match register_audio_pipeline(
        &mut g,
        label,
        cfg.sample_rate as u32,
        cfg.channels as u32,
        buffer_ms as u32,
    ) {
        Ok(id) => id,
        Err(e) => {
            let msg = format!("audio track create failed: {}", e);
            return err(7, &msg);
        }
    };

    let handle = Box::new(LkAudioTrackHandle(AudioTrackHandleRef {
        client: c.0.clone(),
        track_id,
    }));
    unsafe {
        *out_track = Box::into_raw(handle);
    }
    ok()
}

#[no_mangle]
pub extern "C" fn lk_audio_track_destroy(track: *mut LkAudioTrackHandle) -> LkResult {
    if track.is_null() {
        return err(1, "track null");
    }
    unsafe {
        let handle = Box::from_raw(track);
        let client = handle.0.client.clone();
        let track_id = handle.0.track_id;
        drop(handle);

        let mut g = client.lock().unwrap();
        let _ = g.audio_tracks.remove(&track_id);
        if g.default_audio_track_id == Some(track_id) {
            g.default_audio_track_id = None;
        }
    }
    ok()
}

#[no_mangle]
pub extern "C" fn lk_audio_track_publish_pcm_i16(
    track: *mut LkAudioTrackHandle,
    pcm: *const i16,
    frames_per_channel: usize,
) -> LkResult {
    if track.is_null() {
        return err(1, "track null");
    }
    if pcm.is_null() {
        return err(4, "pcm null");
    }
    let handle = unsafe { &*(track as *mut LkAudioTrackHandle) };
    let client = handle.0.client.clone();
    let mut g = client.lock().unwrap();
    let pipeline = match g.audio_tracks.get_mut(&handle.0.track_id) {
        Some(p) => p,
        None => return err(6, "audio track not found"),
    };
    let total = frames_per_channel * pipeline.channels as usize;
    let slice = unsafe { std::slice::from_raw_parts(pcm, total) };
    if let Err(e) = pipeline.push(slice) {
        let msg = format!("audio ring push failed: {}", e);
        return err(8, &msg);
    }
    ok()
}

#[no_mangle]
pub extern "C" fn lk_send_data(
    client: *mut LkClientHandle,
    bytes: *const u8,
    len: usize,
    reliability: LkReliability,
) -> LkResult {
    // Delegate to lk_send_data_ex with default parameters
    lk_send_data_ex(client, bytes, len, reliability, 1, ptr::null())
}

#[no_mangle]
pub extern "C" fn lk_send_data_ex(
    client: *mut LkClientHandle,
    bytes: *const u8,
    len: usize,
    reliability: LkReliability,
    _ordered: c_int, // no effect: reliable data is always ordered, unreliable never is
    label: *const c_char,
) -> LkResult {
    if client.is_null() {
        return err(1, "client null");
    }
    if bytes.is_null() {
        return err(4, "bytes null");
    }
    // Callbacks run on runtime threads, where block_on would panic (and abort the host).
    if tokio::runtime::Handle::try_current().is_ok() {
        return err(206, "lk_send_data cannot be called from an FFI callback; queue the send to your own thread");
    }

    // Size limits (lossy traffic auto-falls back to reliable if payload exceeds MTU)
    const LOSSY_MAX: usize = 1300;
    const RELIABLE_MAX: usize = 15 * 1024;

    struct LogCtx { log_level: LkLogLevel }

    // Gather what the send needs, then release the client lock so a slow send never
    // holds up other FFI calls (#15).
    let c = unsafe { &*(client as *const Client) };
    let (participant, topic, effective_rel, unreliable, rt, stats, timeout_ms, lg) = {
        let g = c.0.lock().unwrap();
        let room = match g.room.as_ref() {
            Some(r) => r,
            None => return err(6, "not connected"),
        };
        // While reconnecting the SDK would park the send until the session is back.
        // Fail fast instead and let the caller drop or queue the data.
        let state = room.connection_state();
        if state != livekit::ConnectionState::Connected {
            match reliability {
                LkReliability::Reliable => g.data_stats.reliable_dropped.fetch_add(1, Ordering::Relaxed),
                LkReliability::Lossy => g.data_stats.lossy_dropped.fetch_add(1, Ordering::Relaxed),
            };
            lk_log!(g, LkLogLevel::Debug, "Send refused: connection state {:?}", state);
            return err(204, &format!("not sent: connection is {:?}", state));
        }

        let mut effective_rel = reliability;
        if matches!(reliability, LkReliability::Lossy) && len > LOSSY_MAX {
            effective_rel = LkReliability::Reliable;
            lk_log!(g, LkLogLevel::Warn,
                "Payload size ({} bytes) exceeds lossy limit ({} bytes); switching to reliable channel",
                len, LOSSY_MAX);
        }
        if matches!(effective_rel, LkReliability::Reliable) && len > RELIABLE_MAX {
            return err(202, &format!("reliable data size {} exceeds limit {}", len, RELIABLE_MAX));
        }

        // Determine topic from label or defaults
        let topic = if !label.is_null() {
            unsafe { cstr(label) }.unwrap_or("custom").to_string()
        } else {
            match effective_rel {
                LkReliability::Reliable => g.data_labels.reliable.clone(),
                LkReliability::Lossy => g.data_labels.lossy.clone(),
            }
        };
        let unreliable = g.lossy_unreliable && matches!(effective_rel, LkReliability::Lossy);
        (
            room.local_participant(),
            topic,
            effective_rel,
            unreliable,
            g.rt.clone(),
            g.data_stats.clone(),
            g.send_timeout_ms,
            LogCtx { log_level: g.log_level },
        )
    };

    let payload = unsafe { std::slice::from_raw_parts(bytes, len) }.to_vec();

    // One byte stream on the reliable, ordered data channel. total_length lets the
    // receiver reject a stream cut short (e.g. by the send timeout) rather than deliver
    // a truncated payload.
    async fn send_stream(participant: &LocalParticipant, topic: &str, payload: &[u8]) -> Result<()> {
        let options = StreamByteOptions {
            topic: topic.to_string(),
            total_length: Some(payload.len() as u64),
            ..Default::default()
        };
        let writer: ByteStreamWriter = participant.stream_bytes(options).await?;
        writer.write(payload).await?;
        writer.close().await?;
        Ok(())
    }

    let send = async {
        if unreliable {
            // Unreliable, unordered, no retransmits (opt-in, see lk_set_lossy_unreliable).
            let packet = DataPacket {
                payload: payload.clone(),
                topic: Some(topic.clone()),
                reliable: false,
                ..Default::default()
            };
            participant.publish_data(packet).await.map_err(anyhow::Error::from)
        } else {
            match send_stream(&participant, &topic, &payload).await {
                Ok(()) => Ok(()),
                Err(e1) => {
                    // Brief backoff then one retry; common when engine is still settling right after join
                    lk_log!(lg, LkLogLevel::Warn, "send_data first attempt failed, retrying: {}", e1);
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    send_stream(&participant, &topic, &payload).await
                }
            }
        }
    };

    let res = rt.block_on(async {
        if timeout_ms == 0 {
            Ok::<_, tokio::time::error::Elapsed>(send.await)
        } else {
            tokio::time::timeout(Duration::from_millis(timeout_ms), send).await
        }
    });

    let count_drop = || match effective_rel {
        LkReliability::Reliable => stats.reliable_dropped.fetch_add(1, Ordering::Relaxed),
        LkReliability::Lossy => stats.lossy_dropped.fetch_add(1, Ordering::Relaxed),
    };
    match res {
        Ok(Ok(())) => {
            match effective_rel {
                LkReliability::Reliable => stats.reliable_sent_bytes.fetch_add(len as i64, Ordering::Relaxed),
                LkReliability::Lossy => stats.lossy_sent_bytes.fetch_add(len as i64, Ordering::Relaxed),
            };
            lk_log!(lg, LkLogLevel::Debug, "Sent data: {} bytes, topic='{}', {}", len, topic, if unreliable { "unreliable packet" } else { "byte stream" });
            ok()
        }
        Ok(Err(e)) => {
            count_drop();
            let msg = format!("send failed: {}", e);
            lk_log!(lg, LkLogLevel::Error, "{}", msg);
            err(203, &msg)
        }
        Err(_) => {
            count_drop();
            let msg = format!("send did not complete within {} ms (reconnecting or congested); not delivered", timeout_ms);
            lk_log!(lg, LkLogLevel::Warn, "{}", msg);
            err(205, &msg)
        }
    }
}

// --------- Statistics Functions ---------

/// # Safety
/// The caller must ensure `out_stats` points to valid writable memory.
#[no_mangle]
pub unsafe extern "C" fn lk_get_audio_stats(
    client: *mut LkClientHandle,
    out_stats: *mut LkAudioStats,
) -> LkResult {
    if client.is_null() {
        return err(1, "client null");
    }
    if out_stats.is_null() {
        return err(4, "out_stats null");
    }
    
    let c = &*(client as *const Client);
    let g = c.0.lock().unwrap();
    
    let mut stats = LkAudioStats {
        sample_rate: 0,
        channels: 0,
        ring_capacity_frames: 0,
        ring_queued_frames: 0,
        underruns: 0,
        overruns: 0,
    };
    if let Some(id) = g.default_audio_track_id {
        if let Some(pipeline) = g.audio_tracks.get(&id) {
            stats.sample_rate = pipeline.sample_rate as c_int;
            stats.channels = pipeline.channels as c_int;
            stats.ring_capacity_frames = pipeline
                .ring
                .capacity_frames
                .min(c_int::MAX as usize) as c_int;
            stats.ring_queued_frames = pipeline
                .ring
                .queued_frames(pipeline.channels)
                .min(c_int::MAX as usize) as c_int;
            stats.underruns = pipeline.ring.underruns.load(Ordering::Relaxed);
            stats.overruns = pipeline.ring.overruns.load(Ordering::Relaxed);
        }
    }
    
    *out_stats = stats;
    
    ok()
}

/// # Safety
/// The caller must ensure `out_stats` points to valid writable memory.
#[no_mangle]
pub unsafe extern "C" fn lk_get_data_stats(
    client: *mut LkClientHandle,
    out_stats: *mut LkDataStats,
) -> LkResult {
    if client.is_null() {
        return err(1, "client null");
    }
    if out_stats.is_null() {
        return err(4, "out_stats null");
    }
    
    let c = &*(client as *const Client);
    let g = c.0.lock().unwrap();
    
    *out_stats = LkDataStats {
        reliable_sent_bytes: g.data_stats.reliable_sent_bytes.load(Ordering::Relaxed),
        reliable_dropped: g.data_stats.reliable_dropped.load(Ordering::Relaxed),
        lossy_sent_bytes: g.data_stats.lossy_sent_bytes.load(Ordering::Relaxed),
        lossy_dropped: g.data_stats.lossy_dropped.load(Ordering::Relaxed),
    };
    
    ok()
}
