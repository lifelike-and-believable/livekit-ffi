# LiveKit FFI API Guide

This guide documents the enhanced LiveKit FFI C API for Unreal Engine and O3DS integration.

## Table of Contents
1. [Quick Start](#quick-start)
2. [Audio Configuration](#audio-configuration)
3. [Data Channel Usage](#data-channel-usage)
   - [Participant Events](#participant-events)
4. [Connection Lifecycle](#connection-lifecycle)
5. [Diagnostics and Monitoring](#diagnostics-and-monitoring)
6. [Error Handling](#error-handling)
7. [Threading Model](#threading-model)

## Quick Start

### Basic Connection (No Changes Required)

```c
// Existing code continues to work unchanged
LkClientHandle* client = lk_client_create();

LkResult result = lk_connect(client, "wss://your-server.com", "your-token");
if (result.code != 0) {
    printf("Connection failed: %s\n", result.message);
    lk_free_str((char*)result.message);
}

// ... use client ...

lk_disconnect(client);
lk_client_destroy(client);
```

## Audio Configuration

### Configure Audio Publishing

Control encoder bitrate, DTX (Discontinuous Transmission), and stereo/mono mode:

```c
LkClientHandle* client = lk_client_create();

// Set audio publish options before connecting
lk_set_audio_publish_options(client, 
    32000,  // bitrate in bps (24000-48000 typical)
    1,      // enable DTX (0=off, 1=on)
    0       // mono (0=mono, 1=stereo)
);

// Now connect and publish audio as usual
lk_connect(client, url, token);
lk_publish_audio_pcm_i16(client, pcm_data, frames, channels, sample_rate);
```

### Configure Audio Subscription Format

Request a specific output format for subscribed audio:

```c
// Set desired output format before subscribing
lk_set_audio_output_format(client, 48000, 1);  // 48kHz mono

// Set audio callback
lk_client_set_audio_callback(client, on_audio_frame, user_data);

// Audio frames will be resampled/downmixed to the requested format
```

### Audio Format Change Notifications

Get notified when the source audio format changes:

```c
void on_format_change(void* user, int32_t sample_rate, int32_t channels) {
    printf("Audio format changed: %dHz, %d channels\n", sample_rate, channels);
}

lk_set_audio_format_change_callback(client, on_format_change, user_data);
```

## Data Channel Usage

### Basic Data Sending (No Changes Required)

```c
// Existing code works unchanged
uint8_t data[] = { 0x01, 0x02, 0x03 };
lk_send_data(client, data, sizeof(data), LkReliable);
```

### Extended Data Sending with Labels

```c
// Send with custom label
uint8_t pose_data[512];
lk_send_data_ex(client, pose_data, sizeof(pose_data), 
    LkReliable,    // reliability
    1,             // ordered: no effect, kept for ABI compatibility
    "pose-update"  // custom label (or NULL for default)
);

// Size limits:
// - Lossy: payloads over 1300 bytes are sent as reliable data instead
// - Reliable: max 15 KiB (returns error 202 if exceeded)
```

### Sends Never Stall the Caller for Long

`lk_send_data` / `lk_send_data_ex` wait only until the data is handed to the
transport, and never hold up other FFI calls on the client:

- While the connection is reconnecting they return **204** at once, without sending.
  Drop or queue the data and resend after `LkConnConnected`.
- Otherwise the send is bounded by `lk_set_send_timeout_ms` (default 1000 ms;
  `0` = unbounded, the pre-0.4 behaviour) and returns **205** on timeout. The first
  sends after connecting also wait for the publisher connection to come up.

After **205** (or **203**, send failed) the outcome is unknown: the data may still
have arrived. Never resend the same delta; send full state instead.

```c
lk_set_send_timeout_ms(client, 250);   // tighter bound for a game thread

LkResult r = lk_send_data_ex(client, frame, len, LkReliable, 1, "pose");
if (r.code == 204 || r.code == 205) {
    need_full_sync = true;   // resend full state once reconnected
}
```

### Lossy Data

By default `LkLossy` data travels exactly like `LkReliable` data: as a byte stream
on the reliable, ordered channel. Every receiver version gets it, but it pays the
reliable channel's retransmission and head-of-line blocking.

To send `LkLossy` as truly unreliable packets (unordered, no retransmission), opt in:

```c
lk_set_lossy_unreliable(client, 1);
lk_send_data_ex(client, mocap, len, LkLossy, 0, "mocap-live");
```

Receivers report these packets with `reliability == LkLossy`. **Receivers built
from livekit_ffi older than 0.4 do not deliver them**, so enable this only when
every receiver is on 0.4 or newer.

### Data Delivery Contract

| | Reliable (`LkReliable`, and `LkLossy` by default) | Unreliable (`LkLossy` + `lk_set_lossy_unreliable`) |
|---|---|---|
| Transport | One byte stream per send, reliable ordered channel | One data packet, lossy channel |
| Order | Send order, per sender | None |
| Loss | None while the session stays up, except: a stream that arrives incomplete or does not finish within 2 s is dropped at the receiver, and neither side is told | Any packet may be lost |
| Integrity | Whole payloads only, never delivered truncated | Whole packets |

A send that returns `0` has been handed to the transport. That is not an
acknowledgement that any receiver has it. `204` means not sent; `203` and `205`
mean the outcome is unknown.

**Reconnects.** The connection callback reports `LkConnReconnecting`, then
`LkConnConnected` when the session is back (or `LkConnDisconnected`).

- Sends made while reconnecting return 204 and are not sent.
- Data already in flight when the reconnect started is not reported to either side
  and may be lost.
- The SDK either resumes the session or replaces it (a full reconnect). The FFI
  cannot tell which happened, and across a full reconnect in-flight data is lost.

**So treat every `LkConnReconnecting` → `LkConnConnected` as a possible gap.** A
sender whose frames depend on earlier frames (for example deltas) should send a full
state after `LkConnConnected` before resuming deltas. A full reconnect also reports
every remote participant Left and then Joined (see
[Participant Events](#participant-events)), the same signal as a new peer joining.

### Custom Default Labels

```c
// Set custom default labels for reliable/lossy channels
lk_set_default_data_labels(client, "my-reliable-channel", "my-lossy-channel");

// Now lk_send_data uses these labels
lk_send_data(client, data, len, LkReliable);  // Uses "my-reliable-channel"
```

### Extended Data Callback

Receive label and reliability information:

```c
void on_data_ex(void* user, const char* label, LkReliability reliability, 
                const uint8_t* bytes, size_t len) {
    printf("Received %zu bytes on '%s' (%s)\n", 
        len, label, reliability == LkReliable ? "reliable" : "lossy");
}

lk_client_set_data_callback_ex(client, on_data_ex, user_data);
```

`label` is never NULL (`""` if the sender gave none), and `reliability` is the
channel the packet actually arrived on. The callback behaves the same after
`lk_connect*` and `lk_connect*_async`. If both data callbacks are set, only the
extended one is called.

## Participant Events

Get told when remote participants join or leave, for example to send a full state
to a late joiner right away:

```c
void on_participant(void* user, LkParticipantEvent ev,
                    const char* identity, const char* name) {
    // identity/name are valid only during this call: copy them.
    // Don't call lk_* here: hand the work to your own thread.
    enqueue_peer_event(user, ev == LkParticipantJoined, identity);
}

lk_set_participant_callback(client, on_participant, user_data);  // before connecting
lk_connect(client, url, token);
```

- Only remote participants are reported, never the local one.
- Participants already in the room at connect are each reported as
  `LkParticipantJoined`, right after `LkConnConnected`.
- Each identity strictly alternates Joined / Left, with no duplicates.
- `name` may be `""`.
- A full reconnect reports every remote participant Left and then Joined.

## Connection Lifecycle

### Monitor Connection State

```c
void on_connection_state(void* user, LkConnectionState state, 
                         int32_t reason_code, const char* message) {
    switch (state) {
        case LkConnConnecting:
            printf("Connecting...\n");
            break;
        case LkConnConnected:
            printf("Connected!\n");
            break;
        case LkConnReconnecting:
            printf("Reconnecting...\n");
            break;
        case LkConnDisconnected:
            printf("Disconnected: %s\n", message ? message : "normal");
            break;
        case LkConnFailed:
            printf("Connection failed (code %d): %s\n", 
                reason_code, message ? message : "unknown");
            break;
    }
}

lk_set_connection_callback(client, on_connection_state, user_data);
lk_connect(client, url, token);
```

### Connection State Management

```c
// Check if connected
if (lk_client_is_ready(client)) {
    // Client is connected and ready
}

// Graceful disconnect (waits for callbacks to complete)
lk_disconnect(client);
// If a room was connected, on_connection_state has now received
// LkConnDisconnected ("ClientInitiated") on this thread; nothing fires after.
```

## Diagnostics and Monitoring

### Audio Statistics

Monitor audio ring buffer health:

```c
LkAudioStats audio_stats;
lk_get_audio_stats(client, &audio_stats);

printf("Audio: %dHz, %dch\n", audio_stats.sample_rate, audio_stats.channels);
printf("Ring: %d/%d frames queued\n", 
    audio_stats.ring_queued_frames, 
    audio_stats.ring_capacity_frames);
printf("Underruns: %d, Overruns: %d\n", 
    audio_stats.underruns, 
    audio_stats.overruns);

// High underruns: audio thread not feeding fast enough
// High overruns: ring buffer too small or audio thread too fast
```

### Data Statistics

Track data channel performance:

```c
LkDataStats data_stats;
lk_get_data_stats(client, &data_stats);

printf("Reliable: %lld bytes sent, %lld dropped\n",
    data_stats.reliable_sent_bytes,
    data_stats.reliable_dropped);
printf("Lossy: %lld bytes sent, %lld dropped\n",
    data_stats.lossy_sent_bytes,
    data_stats.lossy_dropped);
```

### Logging

Control log verbosity:

```c
lk_set_log_level(client, LkLogDebug);  // Error, Warn, Info, Debug, Trace
```

## Error Handling

### Error Code Taxonomy

```c
LkResult result = lk_send_data(client, data, size, LkReliable);
if (result.code != 0) {
    // Error code ranges:
    // 1xx: Connection/Token errors (e.g., not connected)
    // 2xx: Data send errors
    //   202: Reliable data too large (> 15 KiB)
    //   203: Send operation failed
    //   204: Not sent: connection is reconnecting (returned immediately)
    //   205: Send timeout elapsed (lk_set_send_timeout_ms); may have been delivered
    //   206: Called from an FFI callback thread
    // 3xx: Audio publish errors
    // 4xx: Lifecycle errors
    //   403: lk_disconnect called from an FFI callback thread
    // 5xx: Internal/unsupported errors (e.g., 501 = not supported)
    
    printf("Error %d: %s\n", result.code, result.message);
    lk_free_str((char*)result.message);  // Always free error messages
}
```

### Best Practices

1. **Always check return codes**:
   ```c
   LkResult r = lk_connect(client, url, token);
   if (r.code != 0) { /* handle error */ }
   ```

2. **Free error messages**:
   ```c
   if (result.message) {
       lk_free_str((char*)result.message);
   }
   ```

3. **Respect size limits**:
   ```c
   // Lossy payloads over 1300 bytes silently become reliable data
   if (size > 1300 && reliability == LkLossy) {
       // Split if it must stay lossy
   }
   ```

## Threading Model

### Callback Threading

**All callbacks may be invoked on background threads:**

```c
void on_audio_frame(void* user, const int16_t* pcm, size_t frames, 
                    int32_t channels, int32_t sample_rate) {
    // ⚠️ This runs on a background thread!
    // - Do NOT block or perform long operations
    // - Do NOT call sleep(), wait for locks, or do heavy processing
    // - Copy data quickly and return
    
    // ✅ Good: Quick copy and signal
    memcpy(my_buffer, pcm, frames * channels * sizeof(int16_t));
    signal_event();
    
    // ❌ Bad: Blocking operations
    // pthread_mutex_lock(&slow_lock);  // DON'T DO THIS
    // process_heavy_dsp(pcm);          // DON'T DO THIS
}
```

**Don't call `lk_*` functions from inside a callback.** Queue the work to your own
thread. `lk_send_data*` and `lk_disconnect` detect a callback thread and return
206 / 403 instead of blocking.

### API Thread Safety

**All API functions are thread-safe:**

```c
// Safe to call from any thread, even concurrently
void audio_thread() {
    lk_publish_audio_pcm_i16(client, pcm, frames, ch, sr);
}

void data_thread() {
    lk_send_data(client, data, len, LkReliable);
}

void ui_thread() {
    LkAudioStats stats;
    lk_get_audio_stats(client, &stats);
}
```

### Shutdown Guarantees

```c
// After disconnect/destroy returns, NO callbacks will fire
lk_disconnect(client);       // Blocks until callbacks quiesced
// NOW SAFE: No more callbacks

lk_client_destroy(client);   // Also blocks until callbacks quiesced, and closes the room
```

## Advanced Features

### Token Refresh

**Note:** Token refresh at runtime is not currently supported by the underlying LiveKit SDK.

```c
// This will return error 501 (not supported)
LkResult r = lk_refresh_token(client, new_token);
if (r.code == 501) {
    // Fallback: disconnect and reconnect with new token
    lk_disconnect(client);
    lk_connect(client, url, new_token);
}
```

### Dynamic Role Switching

**Note:** Role switching without reconnect is not currently supported.

```c
// This will return error 501 (not supported)
LkResult r = lk_set_role(client, LkRolePublisher, 0);
if (r.code == 501) {
    // Fallback: disconnect and reconnect with new role
    lk_disconnect(client);
    lk_connect_with_role(client, url, token, LkRolePublisher);
}
```

### Reconnection Backoff

**Note:** This is a placeholder for future SDK support.

```c
// Currently a no-op; SDK manages reconnection internally
lk_set_reconnect_backoff(client, 100, 5000, 1.5);
```

## Migration Guide

### From Original API

**No changes required!** All original functions work unchanged:

```c
// All of this continues to work exactly as before:
LkClientHandle* client = lk_client_create();
lk_client_set_data_callback(client, on_data, user);
lk_client_set_audio_callback(client, on_audio, user);
lk_connect(client, url, token);
lk_publish_audio_pcm_i16(client, pcm, frames, ch, sr);
lk_send_data(client, data, len, LkReliable);
lk_disconnect(client);
lk_client_destroy(client);
```

### Adding New Features

Simply add calls to new functions as needed:

```c
LkClientHandle* client = lk_client_create();

// NEW: Configure before connecting
lk_set_audio_publish_options(client, 32000, 1, 0);
lk_set_connection_callback(client, on_connection, user);

// Original workflow continues unchanged
lk_connect(client, url, token);
lk_publish_audio_pcm_i16(client, pcm, frames, ch, sr);

// NEW: Monitor statistics periodically
LkAudioStats stats;
lk_get_audio_stats(client, &stats);
```

## Complete Example

```c
#include "livekit_ffi.h"
#include <stdio.h>
#include <string.h>

void on_connection(void* user, LkConnectionState state, int32_t code, const char* msg) {
    printf("Connection state: %d\n", state);
}

void on_audio(void* user, const int16_t* pcm, size_t frames, int32_t ch, int32_t sr) {
    // Process audio quickly - we're on a background thread
    // Copy to ring buffer or process with minimal latency
}

void on_data_ex(void* user, const char* label, LkReliability rel, 
                const uint8_t* bytes, size_t len) {
    printf("Received %zu bytes on '%s'\n", len, label);
}

int main() {
    LkClientHandle* client = lk_client_create();
    
    // Configure
    lk_set_audio_publish_options(client, 32000, 1, 0);
    lk_set_audio_output_format(client, 48000, 1);
    lk_set_log_level(client, LkLogInfo);
    
    // Set callbacks
    lk_set_connection_callback(client, on_connection, NULL);
    lk_client_set_audio_callback(client, on_audio, NULL);
    lk_client_set_data_callback_ex(client, on_data_ex, NULL);
    
    // Connect
    LkResult result = lk_connect(client, "wss://server.com", "token");
    if (result.code != 0) {
        printf("Connect failed: %s\n", result.message);
        lk_free_str((char*)result.message);
        lk_client_destroy(client);
        return 1;
    }
    
    // Main loop
    while (lk_client_is_ready(client)) {
        // Publish audio
        int16_t audio[480];  // 10ms @ 48kHz
        // ... fill audio ...
        lk_publish_audio_pcm_i16(client, audio, 480, 1, 48000);
        
        // Send data
        uint8_t mocap[256];
        // ... fill mocap ...
        lk_send_data_ex(client, mocap, sizeof(mocap), LkLossy, 1, "mocap");
        
        // Check stats periodically
        LkAudioStats stats;
        lk_get_audio_stats(client, &stats);
        if (stats.underruns > 0 || stats.overruns > 0) {
            printf("Audio issues: underruns=%d, overruns=%d\n", 
                stats.underruns, stats.overruns);
        }
        
        // Sleep or yield
        usleep(10000);  // 10ms
    }
    
    // Cleanup
    lk_disconnect(client);
    lk_client_destroy(client);
    return 0;
}
```

## Troubleshooting

### Audio Underruns

**Symptom:** `lk_get_audio_stats` shows high underrun count

**Causes:**
- Not calling `lk_publish_audio_pcm_i16` frequently enough
- Network issues causing frame drops

**Solutions:**
- Call publish every 10-20ms consistently
- Increase buffer size if needed

### Audio Overruns

**Symptom:** `lk_get_audio_stats` shows high overrun count

**Causes:**
- Publishing audio faster than consumer can process
- Ring buffer too small

**Solutions:**
- Reduce publish frequency
- Increase ring buffer capacity (rebuild required)

### Data Drops

**Symptom:** `lk_get_data_stats` shows dropped packets

**Causes:**
- Network congestion
- Sending too much data
- Exceeding size limits

**Solutions:**
- Reduce data rate for lossy channel
- Use reliable channel for important data
- Split large payloads into smaller chunks

### Connection Issues

**Symptom:** Connection callback shows Reconnecting/Failed states

**Causes:**
- Network interruption
- Invalid token
- Server unavailable

**Solutions:**
- Check network connectivity
- Verify token is valid and not expired
- Monitor connection callback for state transitions
- Implement reconnection logic in your app

## Performance Tips

1. **Audio**: Publish 10-20ms chunks @ 48kHz for optimal latency/throughput balance
2. **Data**: Keep lossy packets ≤ 1000 bytes for best reliability
3. **Statistics**: Query stats every 1-5 seconds, not every frame
4. **Callbacks**: Copy data quickly and process off the callback thread
5. **Threading**: Don't block in callbacks - use lock-free queues if possible

## Further Reading

- [LiveKit Documentation](https://docs.livekit.io/)
- [Original FFI README](../README.md)
- [Local Server Setup](LOCAL_LIVEKIT_QUICKSTART.md)
