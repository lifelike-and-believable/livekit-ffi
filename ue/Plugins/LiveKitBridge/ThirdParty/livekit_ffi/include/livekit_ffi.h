#pragma once
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

// ═══════════════════════════════════════════════════════════════════════════
// Core Types
// ═══════════════════════════════════════════════════════════════════════════

/**
 * Result type for FFI functions.
 * - code: 0 = success; non-zero = error
 * - message: allocated error string (caller must free via lk_free_str), or NULL
 *
 * Error code ranges:
 * - 1xx: Connection/Token errors
 * - 2xx: Data send errors
 *     202 reliable payload larger than 15 KiB
 *     203 send failed; outcome unknown (may have been delivered)
 *     204 not sent: the connection is reconnecting (returned at once, see lk_send_data_ex)
 *     205 send did not finish within the send timeout (lk_set_send_timeout_ms);
 *         outcome unknown (may have been delivered)
 *     206 called from an FFI callback thread (queue the send to your own thread)
 * - 3xx: Audio publish errors
 * - 4xx: Lifecycle errors
 * - 5xx: Internal errors
 */
typedef struct { int32_t code; const char* message; } LkResult;

/**
 * Free a string allocated by the FFI layer.
 * Safe to call with NULL.
 */
void lk_free_str(char* p);

/**
 * Opaque client handle.
 */
typedef struct LkClientHandle LkClientHandle;

/**
 * Opaque audio track handle for publisher-created tracks.
 */
typedef struct LkAudioTrackHandle LkAudioTrackHandle;

/**
 * Data channel reliability mode.
 */
typedef enum { LkReliable = 0, LkLossy = 1 } LkReliability;

/**
 * Client role for connection.
 */
typedef enum {
  LkRoleAuto = 0,
  LkRolePublisher = 1,
  LkRoleSubscriber = 2,
  LkRoleBoth = 3
} LkRole;

/**
 * Connection state enum for lifecycle tracking.
 */
typedef enum {
  LkConnConnecting = 0,
  LkConnConnected = 1,
  LkConnReconnecting = 2,
  LkConnDisconnected = 3,
  LkConnFailed = 4
} LkConnectionState;

/**
 * Remote participant presence change (lk_set_participant_callback).
 */
typedef enum {
  LkParticipantJoined = 0,
  LkParticipantLeft = 1
} LkParticipantEvent;

/**
 * Log level for diagnostics.
 */
typedef enum {
  LkLogError = 0,
  LkLogWarn = 1,
  LkLogInfo = 2,
  LkLogDebug = 3,
  LkLogTrace = 4
} LkLogLevel;

// ═══════════════════════════════════════════════════════════════════════════
// Callbacks
// ═══════════════════════════════════════════════════════════════════════════

/**
 * Data callback (original, no label/reliability info).
 * If both data callbacks are set, only the extended one is called; no packet reaches both.
 * NOTE: Callbacks may be invoked on background threads. Never block internally.
 */
typedef void (*LkDataCallback)(void* user, const uint8_t* bytes, size_t len);

/**
 * Extended data callback with label and reliability.
 * - label: the sender's label (topic); "" if the sender gave none. Never NULL.
 * - reliability: the channel the packet actually arrived on. LkLossy only for packets
 *   sent unreliably (see lk_set_lossy_unreliable); everything else is LkReliable.
 * Delivered the same way after lk_connect* and lk_connect*_async.
 * NOTE: Callbacks may be invoked on background threads. Never block internally.
 */
typedef void (*LkDataCallbackEx)(void* user, const char* label, LkReliability reliability, const uint8_t* bytes, size_t len);

/**
 * Audio callback (PCM i16 interleaved).
 * NOTE: Callbacks may be invoked on background threads. Never block internally.
 */
typedef void (*LkAudioCallback)(void* user, const int16_t* pcm_interleaved, size_t frames_per_channel, int32_t channels, int32_t sample_rate);

/**
 * Extended audio callback with per-subject identification.
 * Provides participant name and track name for each audio frame.
 * - participant_name: name of the participant (never NULL)
 * - track_name: name of the audio track (never NULL)
 * NOTE: Callbacks may be invoked on background threads. Never block internally.
 */
typedef void (*LkAudioCallbackEx)(void* user, const int16_t* pcm_interleaved, size_t frames_per_channel, int32_t channels, int32_t sample_rate, const char* participant_name, const char* track_name);

/**
 * Audio format change notification callback.
 * Called when the incoming audio format changes.
 * NOTE: Callbacks may be invoked on background threads. Never block internally.
 */
typedef void (*LkAudioFormatChangeCallback)(void* user, int32_t sample_rate, int32_t channels);

/**
 * Connection state change callback.
 * - state: new connection state
 * - reason_code: error/disconnect reason (0 if normal)
 * - message: optional human-readable message (may be NULL)
 * NOTE: Callbacks may be invoked on background threads. Never block internally.
 */
typedef void (*LkConnectionCallback)(void* user, LkConnectionState state, int32_t reason_code, const char* message);

/**
 * Remote participant joined / left callback.
 * - identity: participant identity (never NULL)
 * - name: participant display name; may be "" (never NULL)
 * Both strings are valid only for the duration of the call; copy them if needed.
 * Only remote participants are reported, never the local one. Participants already in
 * the room when the connection is established are each reported as LkParticipantJoined
 * right after LkConnConnected. Each identity alternates strictly Joined / Left.
 * During a full reconnect the SDK drops and re-adds every remote participant, so each
 * one is reported Left and then Joined again.
 * NOTE: Callbacks may be invoked on background threads. Never block internally.
 */
typedef void (*LkParticipantCallback)(void* user, LkParticipantEvent event, const char* identity, const char* name);

// ═══════════════════════════════════════════════════════════════════════════
// Diagnostic Structures
// ═══════════════════════════════════════════════════════════════════════════

/**
 * Audio statistics for diagnostics.
 */
typedef struct {
  int32_t sample_rate;
  int32_t channels;
  int32_t ring_capacity_frames;
  int32_t ring_queued_frames;
  int32_t underruns;
  int32_t overruns;
} LkAudioStats;

/**
 * Data channel statistics for diagnostics.
 */
typedef struct {
  int64_t reliable_sent_bytes;
  int64_t reliable_dropped;
  int64_t lossy_sent_bytes;
  int64_t lossy_dropped;
} LkDataStats;

// ═══════════════════════════════════════════════════════════════════════════
// Client Lifecycle
// ═══════════════════════════════════════════════════════════════════════════

/**
 * Create a new client handle.
 */
LkClientHandle* lk_client_create(void);

/**
 * Destroy a client handle and free resources.
 * After this call returns, no callbacks will fire.
 */
void lk_client_destroy(LkClientHandle*);

/**
 * Set data callback (original).
 */
LkResult lk_client_set_data_callback(LkClientHandle*, LkDataCallback cb, void* user);

/**
 * Set extended data callback (with label and reliability).
 */
LkResult lk_client_set_data_callback_ex(LkClientHandle*, LkDataCallbackEx cb, void* user);

/**
 * Set audio callback.
 */
LkResult lk_client_set_audio_callback(LkClientHandle*, LkAudioCallback cb, void* user);

/**
 * Set extended audio callback with per-subject identification.
 * Provides participant and track names for each audio frame.
 * Overrides any previously set standard audio callback.
 */
LkResult lk_client_set_audio_callback_ex(LkClientHandle*, LkAudioCallbackEx cb, void* user);

/**
 * Set audio format change callback.
 * Called when incoming audio format changes.
 */
LkResult lk_set_audio_format_change_callback(LkClientHandle*, LkAudioFormatChangeCallback cb, void* user);

/**
 * Set connection state callback.
 */
LkResult lk_set_connection_callback(LkClientHandle*, LkConnectionCallback cb, void* user);

/**
 * Set remote participant joined/left callback.
 * Set it before connecting: participants already in the room are announced once,
 * immediately after the connection is established.
 */
LkResult lk_set_participant_callback(LkClientHandle*, LkParticipantCallback cb, void* user);

/**
 * Connect to LiveKit room (defaults to LkRoleBoth).
 */
LkResult lk_connect(LkClientHandle*, const char* url, const char* token);

/**
 * Connect to LiveKit room with specified role.
 */
LkResult lk_connect_with_role(LkClientHandle*, const char* url, const char* token, LkRole role);

/**
 * Asynchronously connect to LiveKit room (defaults to LkRoleBoth).
 * Returns immediately; connection result will be delivered via lk_set_connection_callback.
 */
LkResult lk_connect_async(LkClientHandle*, const char* url, const char* token);

/**
 * Asynchronously connect to LiveKit room with specified role.
 * Returns immediately; connection result will be delivered via lk_set_connection_callback.
 */
LkResult lk_connect_with_role_async(LkClientHandle*, const char* url, const char* token, LkRole role);

/**
 * Disconnect from LiveKit room.
 * Blocks until disconnect is complete and callbacks are quiesced. If a room was
 * connected, the connection callback receives LkConnDisconnected (message
 * "ClientInitiated") on the calling thread before this returns; no callback fires after.
 * Must not be called from inside an FFI callback (returns code 403).
 */
LkResult lk_disconnect(LkClientHandle*);

/**
 * Check if client is connected and ready.
 * Returns 1 if ready, 0 otherwise.
 */
int32_t lk_client_is_ready(LkClientHandle*);

// ═══════════════════════════════════════════════════════════════════════════
// Audio Configuration
// ═══════════════════════════════════════════════════════════════════════════

/**
 * Configure audio publish options.
 * - bitrate_bps: target bitrate in bits per second (e.g., 24000-48000)
 * - enable_dtx: 1 to enable Discontinuous Transmission, 0 to disable
 * - stereo: 1 for stereo, 0 for mono (default mono)
 *
 * Call before first audio publish or disconnect/reconnect to apply changes.
 */
LkResult lk_set_audio_publish_options(LkClientHandle*, int32_t bitrate_bps, int32_t enable_dtx, int32_t stereo);

/**
 * Set desired audio output format for subscribed audio.
 * The FFI layer will resample/downmix incoming audio to this format.
 * - sample_rate: desired sample rate (e.g., 48000)
 * - channels: desired channel count (1 or 2)
 *
 * Call before connecting or subscribing to audio.
 */
LkResult lk_set_audio_output_format(LkClientHandle*, int32_t sample_rate, int32_t channels);

// ═══════════════════════════════════════════════════════════════════════════
// Audio Publishing
// ═══════════════════════════════════════════════════════════════════════════

/**
 * Publish PCM i16 audio frame.
 * - pcm_interleaved: interleaved i16 samples
 * - frames_per_channel: number of frames per channel
 * - channels: channel count
 * - sample_rate: sample rate in Hz
 */
LkResult lk_publish_audio_pcm_i16(
  LkClientHandle*,
  const int16_t* pcm_interleaved,
  size_t frames_per_channel,
  int32_t channels,
  int32_t sample_rate);

/**
 * Audio track configuration for dedicated publisher tracks.
 * - track_name: optional track label (NULL uses default)
 * - sample_rate / channels: format for the track
 * - buffer_ms: desired ring buffer depth in milliseconds (0 = default)
 */
typedef struct {
  const char* track_name;
  int32_t sample_rate;
  int32_t channels;
  int32_t buffer_ms;
} LkAudioTrackConfig;

/**
 * Create a dedicated audio track for publishing.
 * Returns the track handle via out param on success.
 */
LkResult lk_audio_track_create(
  LkClientHandle*,
  const LkAudioTrackConfig* config,
  LkAudioTrackHandle** out_track);

/**
 * Destroy a dedicated audio track handle and stop publishing it.
 *
 * Safe to call with NULL (no-op).
 */
LkResult lk_audio_track_destroy(LkAudioTrackHandle*);

/**
 * Publish PCM audio to a dedicated audio track handle.
 * Format is determined by the track's configuration.
 */
LkResult lk_audio_track_publish_pcm_i16(
  LkAudioTrackHandle*,
  const int16_t* pcm_interleaved,
  size_t frames_per_channel);

// ═══════════════════════════════════════════════════════════════════════════
// Data Channel
// ═══════════════════════════════════════════════════════════════════════════

/**
 * Send data (original API). Same as lk_send_data_ex(..., ordered = 1, label = NULL).
 * Size guidance: lossy ≤ ~1300 bytes, reliable ≤ ~15 KiB.
 */
LkResult lk_send_data(
  LkClientHandle*,
  const uint8_t* bytes,
  size_t len,
  LkReliability reliability);

/**
 * Send data with extended options.
 * - ordered: has no effect; it is kept for ABI compatibility. Ordering follows from
 *   the transport (see "Data delivery contract" below).
 * - label: optional label (topic) delivered to receivers (NULL uses the default label)
 *
 * Size guidance: lossy ≤ ~1300 bytes, reliable ≤ ~15 KiB. A lossy payload over
 * ~1300 bytes is sent as reliable data instead; a reliable payload over 15 KiB
 * returns 202.
 *
 * Blocking: the call waits until the data is handed to the transport, bounded by the
 * send timeout (lk_set_send_timeout_ms, default 1000 ms, then code 205). It never
 * holds up other FFI calls on the client. While the connection is reconnecting it
 * returns 204 at once without sending; drop or queue the data and resend after
 * LkConnConnected. After 203 or 205 the data may or may not have arrived: never
 * resend the same delta, send full state instead. Must not be called from inside an
 * FFI callback (returns 206).
 */
LkResult lk_send_data_ex(
  LkClientHandle*,
  const uint8_t* bytes,
  size_t len,
  LkReliability reliability,
  int32_t ordered,
  const char* label);

/**
 * Set default labels for reliable and lossy data channels.
 * If NULL, uses built-in defaults.
 */
LkResult lk_set_default_data_labels(LkClientHandle*, const char* reliable_label, const char* lossy_label);

/**
 * Choose how LkLossy data is sent.
 * - enable = 0 (default): LkLossy data goes out like LkReliable data, as a byte stream
 *   on the reliable, ordered channel. Every receiver version gets it.
 * - enable = 1: LkLossy data is sent as unreliable, unordered data packets with no
 *   retransmission, so a lost packet is never waited for. Receivers see it with
 *   reliability LkLossy. Receivers built from livekit_ffi older than 0.4 do not
 *   deliver these packets, so enable it only when every receiver is 0.4 or newer.
 */
LkResult lk_set_lossy_unreliable(LkClientHandle*, int32_t enable);

/**
 * Bound how long lk_send_data / lk_send_data_ex may block, in milliseconds.
 * Default 1000. 0 means no bound (the pre-0.4 behaviour, under which a reconnect that
 * starts mid-send blocks the caller until it ends). On timeout the send returns 205;
 * the data may or may not have been delivered.
 * The first sends after connecting also wait for the publisher connection to come up.
 */
LkResult lk_set_send_timeout_ms(LkClientHandle*, int32_t timeout_ms);

// ───────────────────────────────────────────────────────────────────────────
// Data delivery contract
// ───────────────────────────────────────────────────────────────────────────
//
// Reliable data (LkReliable, and LkLossy unless lk_set_lossy_unreliable is on):
// - Each send is one byte stream on the publisher's reliable, ordered data channel.
// - While the session stays up, data from one sender reaches each receiver once, in
//   send order. Receivers read streams in arrival order and deliver each payload
//   whole. One exception: a stream that arrives incomplete, or does not finish within
//   2 s of opening, is dropped at the receiver, and neither side is told (the sender
//   already got 0). Payloads are never delivered truncated.
// - 0 means the data was handed to the transport, not that a receiver has it.
//   204 means it was not sent. 203 and 205 mean the outcome is unknown: it may have
//   arrived, so never resend the same delta; send full state instead.
//
// Unreliable data (LkLossy with lk_set_lossy_unreliable(client, 1)):
// - Single packets on the lossy channel: unordered, no retransmission. Any packet
//   may be lost or reordered.
//
// Reconnects. The connection callback reports LkConnReconnecting, then
// LkConnConnected when the session is back (or LkConnDisconnected if it is not).
// - Sends during LkConnReconnecting return 204 and are not sent.
// - Data in flight when the reconnect started is not reported to either side and may
//   be lost. The SDK can either resume the existing session or replace it (a full
//   reconnect); the FFI cannot tell these apart, and across a full reconnect in-flight
//   data is lost.
// - So treat every LkConnReconnecting -> LkConnConnected as a possible gap: a sender
//   whose data depends on earlier data (e.g. deltas) should send a full state after
//   LkConnConnected. A full reconnect also reports every remote participant Left and
//   then Joined (lk_set_participant_callback), the same signal as a new peer joining.
//
// ───────────────────────────────────────────────────────────────────────────

// ═══════════════════════════════════════════════════════════════════════════
// Reconnection and Token Management
// ═══════════════════════════════════════════════════════════════════════════

/**
 * Set reconnection backoff parameters.
 * - initial_ms: initial backoff in milliseconds
 * - max_ms: maximum backoff in milliseconds
 * - multiplier: backoff multiplier (e.g., 1.5)
 *
 * Call before connecting.
 */
LkResult lk_set_reconnect_backoff(LkClientHandle*, int32_t initial_ms, int32_t max_ms, float multiplier);

/**
 * Refresh JWT token at runtime (if SDK supports it).
 * If not supported, returns error; fallback is disconnect + reconnect.
 */
LkResult lk_refresh_token(LkClientHandle*, const char* token);

/**
 * Set client role dynamically (if SDK supports it).
 * - role: new role
 * - auto_subscribe: 1 to enable auto-subscribe, 0 to disable
 *
 * If not supported, returns error; fallback is disconnect + reconnect.
 */
LkResult lk_set_role(LkClientHandle*, LkRole role, int32_t auto_subscribe);

// ═══════════════════════════════════════════════════════════════════════════
// Diagnostics and Metrics
// ═══════════════════════════════════════════════════════════════════════════

/**
 * Set log level for diagnostics.
 */
LkResult lk_set_log_level(LkClientHandle*, LkLogLevel level);

/**
 * Get audio statistics.
 * Returns current audio ring buffer state and error counters.
 */
LkResult lk_get_audio_stats(LkClientHandle*, LkAudioStats* out_stats);

/**
 * Get data channel statistics.
 * Returns cumulative send/drop counters.
 */
LkResult lk_get_data_stats(LkClientHandle*, LkDataStats* out_stats);

// ═══════════════════════════════════════════════════════════════════════════
// Threading and Safety Guarantees
// ═══════════════════════════════════════════════════════════════════════════
//
// - All callbacks may be invoked on background threads.
// - Callbacks must not block or perform long-running operations.
// - Do not call lk_* functions from inside a callback. Queue the work to your own
//   thread instead (e.g. send a full sync from your game thread after a Joined).
//   lk_send_data* and lk_disconnect detect this and return 206 / 403.
// - API calls are thread-safe and may be called from any thread.
// - After lk_disconnect() or lk_client_destroy() returns, no further callbacks
//   will be invoked.
// - Reentrancy: It is safe to call API functions from non-callback threads
//   while callbacks are in flight.
//
// ═══════════════════════════════════════════════════════════════════════════

#ifdef __cplusplus
}
#endif
