# LiveKit FFI C++ Library - User Guide

## Table of Contents

1. [Introduction](#introduction)
2. [Getting Started](#getting-started)
3. [Installation](#installation)
4. [Basic Usage](#basic-usage)
5. [Core Concepts](#core-concepts)
6. [Audio Streaming](#audio-streaming)
7. [Data Channels](#data-channels)
8. [Connection Management](#connection-management)
9. [Advanced Features](#advanced-features)
10. [Unreal Engine Integration](#unreal-engine-integration)
11. [Best Practices](#best-practices)
12. [Performance Optimization](#performance-optimization)

---

## Introduction

The LiveKit FFI (Foreign Function Interface) library provides a C API for integrating real-time audio and data streaming capabilities into C++ and Unreal Engine applications. It wraps the LiveKit Rust SDK with a clean, thread-safe C interface suitable for game engines and multimedia applications.

### Key Features

- **Real-time Audio Streaming**: Publish and subscribe to PCM audio with automatic resampling and mixing
- **Reliable & Lossy Data Channels**: Send structured data with configurable reliability and ordering
- **Connection Management**: Automatic reconnection, state tracking, and graceful disconnect handling
- **Thread-Safe API**: All functions can be called from any thread safely
- **Low Latency**: Optimized for real-time applications with minimal overhead
- **Cross-Platform**: Works on Windows, macOS, and Linux

### Use Cases

- **Voice Chat**: Real-time audio communication for multiplayer games
- **Motion Capture Streaming**: Low-latency transmission of pose and animation data
- **Live Broadcasting**: Stream game audio and commentary
- **Remote Collaboration**: Share audio and control data between applications

---

## Getting Started

### Prerequisites

Before using the LiveKit FFI library, ensure you have:

1. **LiveKit Server**: A running LiveKit server instance
   - For local development, see [Local Server Quickstart](LOCAL_LIVEKIT_QUICKSTART.md)
   - For production, use [LiveKit Cloud](https://livekit.io/) or self-hosted

2. **Access Token**: A valid JWT token with appropriate permissions
   - See [Token Minting Guide](TOKEN_MINTING.md) for generating tokens

3. **Build Environment**:
   - **Windows**: Visual Studio 2019+ with C++ tools
   - **macOS**: Xcode command line tools
   - **Linux**: GCC 7+ or Clang 10+
   - **Rust**: 1.70+ (for building from source)

### Quick Example

Here's a minimal example to connect and stream audio:

```cpp
#include "livekit_ffi.h"
#include <stdio.h>

void on_connection(void* user, LkConnectionState state, int32_t code, const char* msg) {
    if (state == LkConnConnected) {
        printf("Connected successfully!\n");
    }
}

int main() {
    // Create client
    LkClientHandle* client = lk_client_create();
    
    // Set connection callback
    lk_set_connection_callback(client, on_connection, NULL);
    
    // Connect to room
    LkResult result = lk_connect(client, 
        "wss://your-server.livekit.io", 
        "your-jwt-token");
    
    if (result.code != 0) {
        printf("Connection failed: %s\n", result.message);
        lk_free_str((char*)result.message);
        lk_client_destroy(client);
        return 1;
    }
    
    // Wait for connection to establish
    while (lk_client_is_ready(client)) {
        // Publish audio, send data, etc.
        sleep(1);
    }
    
    // Cleanup
    lk_disconnect(client);
    lk_client_destroy(client);
    return 0;
}
```

---

## Installation

### Option 1: Use Pre-built Binaries (Recommended)

Download pre-built artifacts from GitHub Actions:

1. Go to the [repository's Actions tab](https://github.com/lifelike-and-believable/livekit-ffi-ue/actions)
2. Download the latest `livekit-ffi-sdk-windows-x64` (or your platform) artifact
3. Extract to your project

**Windows SDK Layout:**
```
livekit-ffi-sdk/
├── include/
│   └── livekit_ffi.h       # C header
├── lib/Win64/Release/
│   ├── livekit_ffi.dll.lib # Import library
│   └── livekit_ffi.lib     # Static library (optional)
└── bin/
    ├── livekit_ffi.dll     # Runtime DLL
    └── livekit_ffi.pdb     # Debug symbols
```

### Option 2: Build from Source

#### With LiveKit Backend (Recommended)

```powershell
# Windows (PowerShell)
cd livekit_ffi
cargo build --release --features with_livekit
```

```bash
# Linux/macOS
cd livekit_ffi
cargo build --release --features with_livekit
```

**Requirements:**
- Rust 1.70+
- LLVM/Clang (for libwebrtc_sys)
- On Windows: Visual Studio with C++ tools

#### Stub Mode (Testing Only)

For testing the build toolchain without LiveKit:

```bash
cd livekit_ffi
cargo build --release
```

This produces a no-op library with the same API but no actual networking.

### Build Artifacts

After building, artifacts are located in:
- **DLL/SO**: `livekit_ffi/target/release/livekit_ffi.dll` (Windows) or `livekit_ffi.so` (Linux)
- **Import Lib**: `livekit_ffi/target/release/livekit_ffi.dll.lib` (Windows only)
- **Static Lib**: `livekit_ffi/target/release/livekit_ffi.lib` (all platforms)
- **Header**: `livekit_ffi/include/livekit_ffi.h`

### Integration into Your Project

#### CMake Example

```cmake
# Add include directory
include_directories(${CMAKE_SOURCE_DIR}/third_party/livekit_ffi/include)

# Link against import library (Windows)
if(WIN32)
    target_link_libraries(YourTarget 
        ${CMAKE_SOURCE_DIR}/third_party/livekit_ffi/lib/Win64/Release/livekit_ffi.dll.lib)
    
    # Copy DLL to output directory
    add_custom_command(TARGET YourTarget POST_BUILD
        COMMAND ${CMAKE_COMMAND} -E copy_if_different
        "${CMAKE_SOURCE_DIR}/third_party/livekit_ffi/bin/livekit_ffi.dll"
        $<TARGET_FILE_DIR:YourTarget>)
endif()
```

#### Visual Studio Project

1. Add include path: `Configuration Properties > C/C++ > General > Additional Include Directories`
   - Add: `$(ProjectDir)third_party\livekit_ffi\include`

2. Add library: `Configuration Properties > Linker > Input > Additional Dependencies`
   - Add: `livekit_ffi.dll.lib`

3. Add library path: `Configuration Properties > Linker > General > Additional Library Directories`
   - Add: `$(ProjectDir)third_party\livekit_ffi\lib\Win64\Release`

4. Delay-load (optional): `Configuration Properties > Linker > Input > Delay Loaded DLLs`
   - Add: `livekit_ffi.dll`

5. Copy DLL to output directory or ensure it's in PATH

---

## Basic Usage

### Client Lifecycle

Every LiveKit FFI session follows this lifecycle:

1. **Create** - Initialize a client handle
2. **Configure** - Set callbacks and options
3. **Connect** - Establish connection to room
4. **Use** - Publish audio, send data, receive callbacks
5. **Disconnect** - Gracefully close connection
6. **Destroy** - Free resources

#### 1. Create a Client

```cpp
LkClientHandle* client = lk_client_create();
if (!client) {
    // Handle allocation failure
    return;
}
```

#### 2. Configure Callbacks

Set up callbacks before connecting:

```cpp
// Data callback
void on_data(void* user, const uint8_t* bytes, size_t len) {
    printf("Received %zu bytes\n", len);
    // Process data
}

// Audio callback
void on_audio(void* user, const int16_t* pcm, size_t frames, 
              int32_t channels, int32_t sample_rate) {
    // Process audio samples (pcm is interleaved i16)
}

// Connection state callback
void on_connection(void* user, LkConnectionState state, 
                   int32_t code, const char* msg) {
    switch (state) {
        case LkConnConnecting:
            printf("Connecting...\n");
            break;
        case LkConnConnected:
            printf("Connected!\n");
            break;
        case LkConnFailed:
            printf("Connection failed: %s\n", msg);
            break;
        case LkConnDisconnected:
            printf("Disconnected\n");
            break;
    }
}

// Register callbacks
lk_client_set_data_callback(client, on_data, user_data);
lk_client_set_audio_callback(client, on_audio, user_data);
lk_set_connection_callback(client, on_connection, user_data);
```

#### 3. Connect to Room

**Synchronous Connection:**
```cpp
LkResult result = lk_connect(client, "wss://server.livekit.io", token);
if (result.code != 0) {
    printf("Error: %s\n", result.message);
    lk_free_str((char*)result.message);
    // Handle error
}
```

**Asynchronous Connection (Non-blocking):**
```cpp
LkResult result = lk_connect_async(client, "wss://server.livekit.io", token);
// Returns immediately, monitor via on_connection callback
```

**With Explicit Role:**
```cpp
// Publisher only (won't subscribe to others)
lk_connect_with_role(client, url, token, LkRolePublisher);

// Subscriber only (won't publish)
lk_connect_with_role(client, url, token, LkRoleSubscriber);

// Both (default)
lk_connect_with_role(client, url, token, LkRoleBoth);
```

#### 4. Check Connection State

```cpp
if (lk_client_is_ready(client)) {
    // Client is connected and ready
    // Safe to publish audio and send data
}
```

#### 5. Disconnect

```cpp
// Graceful disconnect (blocks until callbacks complete)
LkResult result = lk_disconnect(client);
// After this returns, no more callbacks will fire
```

#### 6. Destroy Client

```cpp
// Free all resources
lk_client_destroy(client);
// After this, client pointer is invalid
```

---

## Core Concepts

### Error Handling

All API functions return `LkResult`:

```cpp
typedef struct { 
    int32_t code;      // 0 = success, non-zero = error
    const char* message;  // Error message (must be freed)
} LkResult;
```

**Always check return codes and free error messages:**

```cpp
LkResult result = lk_send_data(client, data, len, LkReliable);
if (result.code != 0) {
    fprintf(stderr, "Error %d: %s\n", result.code, result.message);
    lk_free_str((char*)result.message);  // IMPORTANT: Free the string!
    // Handle error
}
```

**Error Code Ranges:**
- `1xx` - Connection/Token errors
- `2xx` - Data send errors (202: reliable too large, 203: send failed, 204: reconnecting, 205: send timeout, 206: called from a callback)
- `3xx` - Audio publish errors
- `4xx` - Lifecycle errors
- `5xx` - Internal errors (501: not supported)

### Thread Safety

**All API functions are thread-safe** and can be called from any thread:

```cpp
// Thread 1: Publishing audio
void audio_thread() {
    while (running) {
        int16_t audio[480];
        capture_audio(audio);
        lk_publish_audio_pcm_i16(client, audio, 480, 1, 48000);
    }
}

// Thread 2: Sending data
void data_thread() {
    while (running) {
        uint8_t data[256];
        prepare_data(data);
        lk_send_data(client, data, sizeof(data), LkLossy);
    }
}

// Thread 3: Monitoring stats
void monitor_thread() {
    while (running) {
        LkAudioStats stats;
        lk_get_audio_stats(client, &stats);
        log_stats(&stats);
        sleep(5);
    }
}
```

### Callback Threading

**⚠️ Important: Callbacks may run on background threads!**

Never block in callbacks:

```cpp
// ✅ GOOD: Quick copy and return
void on_audio(void* user, const int16_t* pcm, size_t frames, 
              int32_t ch, int32_t sr) {
    memcpy(my_buffer, pcm, frames * ch * sizeof(int16_t));
    signal_ready();  // Quick signaling
}

// ❌ BAD: Blocking operations
void on_audio(void* user, const int16_t* pcm, size_t frames,
              int32_t ch, int32_t sr) {
    pthread_mutex_lock(&heavy_lock);  // DON'T BLOCK!
    process_with_heavy_dsp(pcm);      // DON'T DO HEAVY WORK!
    sleep(10);                         // NEVER SLEEP!
}
```

**Best practices for callbacks:**
- Copy data quickly to a lock-free queue
- Signal a condition variable or semaphore
- Return as fast as possible
- Do heavy processing on a separate thread

### Shutdown Guarantees

After `lk_disconnect()` or `lk_client_destroy()` returns, **no callbacks will fire**:

```cpp
lk_disconnect(client);
// NOW SAFE: All callbacks have completed

// Can safely free user data
free(user_data);

lk_client_destroy(client);
```

---

## Audio Streaming

### Publishing Audio

#### Basic Audio Publishing

Publish interleaved PCM i16 samples:

```cpp
// Example: 10ms of mono audio at 48kHz
int16_t audio[480];  // 480 samples = 10ms at 48kHz
fill_audio_buffer(audio);

LkResult result = lk_publish_audio_pcm_i16(
    client,
    audio,      // Interleaved PCM i16
    480,        // Frames per channel
    1,          // Channels (1=mono, 2=stereo)
    48000       // Sample rate
);
```

**Best practices:**
- Publish in 10-20ms chunks for optimal latency
- Use consistent sample rate (48kHz recommended)
- Maintain steady pacing (avoid bursty uploads)

#### Configure Audio Encoding

Set encoder options before publishing:

```cpp
lk_set_audio_publish_options(
    client,
    32000,  // Bitrate (bps): 24000-48000 typical
    1,      // Enable DTX (Discontinuous Transmission)
    0       // Mono (0) or stereo (1)
);
```

**DTX (Discontinuous Transmission):**
- When enabled, silence is not encoded/transmitted
- Reduces bandwidth during silent periods
- Recommended for voice chat

### Subscribing to Audio

#### Set Audio Output Format

Request a specific output format (resampling/mixing applied automatically):

```cpp
// Request 48kHz mono output
lk_set_audio_output_format(client, 48000, 1);
```

#### Receive Audio Frames

Audio arrives via callback:

```cpp
void on_audio(void* user, const int16_t* pcm, size_t frames,
              int32_t channels, int32_t sample_rate) {
    // pcm is interleaved i16: L, R, L, R, ... (for stereo)
    // frames = number of frames per channel
    // Total samples = frames * channels
    
    size_t total_samples = frames * channels;
    int16_t* buffer = (int16_t*)malloc(total_samples * sizeof(int16_t));
    memcpy(buffer, pcm, total_samples * sizeof(int16_t));
    
    // Queue for playback or processing
    audio_queue_push(buffer, frames, channels, sample_rate);
}

lk_client_set_audio_callback(client, on_audio, user_data);
```

#### Extended Audio Callback (with Source Info)

Get participant and track names:

```cpp
void on_audio_ex(void* user, const int16_t* pcm, size_t frames,
                 int32_t ch, int32_t sr, 
                 const char* participant_name, 
                 const char* track_name) {
    printf("Audio from %s (track: %s): %zu frames\n", 
           participant_name, track_name, frames);
    // Process audio with source identification
}

lk_client_set_audio_callback_ex(client, on_audio_ex, user_data);
```

#### Audio Format Change Notifications

Get notified when source format changes:

```cpp
void on_format_change(void* user, int32_t sample_rate, int32_t channels) {
    printf("Audio format changed: %dHz, %dch\n", sample_rate, channels);
    // Reconfigure audio pipeline if needed
}

lk_set_audio_format_change_callback(client, on_format_change, user_data);
```

### Dedicated Audio Tracks

Create multiple audio tracks for different sources:

```cpp
// Create track configuration
LkAudioTrackConfig config = {
    .track_name = "microphone",
    .sample_rate = 48000,
    .channels = 1,
    .buffer_ms = 1000  // 1 second buffer
};

// Create track
LkAudioTrackHandle* track = NULL;
LkResult result = lk_audio_track_create(client, &config, &track);
if (result.code != 0) {
    // Handle error
}

// Publish to specific track
int16_t audio[480];
lk_audio_track_publish_pcm_i16(track, audio, 480);

// Destroy when done
lk_audio_track_destroy(track);
```

### Audio Diagnostics

Monitor audio pipeline health:

```cpp
LkAudioStats stats;
lk_get_audio_stats(client, &stats);

printf("Audio Pipeline Status:\n");
printf("  Format: %dHz, %d channels\n", stats.sample_rate, stats.channels);
printf("  Ring Buffer: %d/%d frames\n", 
       stats.ring_queued_frames, stats.ring_capacity_frames);
printf("  Underruns: %d\n", stats.underruns);
printf("  Overruns: %d\n", stats.overruns);
```

**Interpreting stats:**
- **High underruns**: Audio thread not feeding fast enough
- **High overruns**: Publishing too fast or buffer too small
- **Ring nearly full**: Consumer (network) may be slow

---

## Data Channels

LiveKit provides two data channel modes:

| Mode | Max Size | Use Case |
|------|----------|----------|
| **Reliable** | ~15 KiB | Critical data, state updates, RPC |
| **Lossy** | ~1300 bytes | High-frequency updates, motion capture |

### Sending Data

#### Basic Send

```cpp
uint8_t data[256];
prepare_data(data);

LkResult result = lk_send_data(client, data, sizeof(data), LkReliable);
if (result.code != 0) {
    fprintf(stderr, "Send failed: %s\n", result.message);
    lk_free_str((char*)result.message);
}
```

#### Extended Send (with Label and Ordering)

```cpp
uint8_t pose_data[512];

LkResult result = lk_send_data_ex(
    client,
    pose_data,
    sizeof(pose_data),
    LkLossy,        // Reliability
    1,              // Ordered (1) or unordered (0)
    "pose-stream"   // Custom label
);
```

**Size Limits:**
- Lossy: Max 1300 bytes (larger payloads are sent as reliable data instead)
- Reliable: Max ~15 KiB (error 202 if exceeded)

**Best practices:**
- Keep lossy packets ≤ 1000 bytes for best delivery
- Use reliable for critical data only
- Use lossy for high-frequency updates where loss is acceptable

#### Custom Default Labels

Set custom channel labels:

```cpp
lk_set_default_data_labels(client, 
    "my-reliable-channel",  // Reliable label
    "my-lossy-channel"      // Lossy label
);

// Now lk_send_data uses these labels
lk_send_data(client, data, len, LkReliable);  // Uses "my-reliable-channel"
```

### Receiving Data

#### Basic Callback

```cpp
void on_data(void* user, const uint8_t* bytes, size_t len) {
    printf("Received %zu bytes\n", len);
    // Process bytes
    
    // Parse example
    if (len >= sizeof(MyHeader)) {
        MyHeader* header = (MyHeader*)bytes;
        // Process structured data
    }
}

lk_client_set_data_callback(client, on_data, user_data);
```

#### Extended Callback (with Metadata)

```cpp
void on_data_ex(void* user, const char* label, LkReliability reliability,
                const uint8_t* bytes, size_t len) {
    printf("Received %zu bytes on '%s' (%s)\n", 
           len, label,
           reliability == LkReliable ? "reliable" : "lossy");
    
    // Route based on label
    if (strcmp(label, "pose-stream") == 0) {
        process_pose_data(bytes, len);
    } else if (strcmp(label, "game-state") == 0) {
        process_game_state(bytes, len);
    }
}

lk_client_set_data_callback_ex(client, on_data_ex, user_data);
```

### Data Channel Diagnostics

Track data channel performance:

```cpp
LkDataStats stats;
lk_get_data_stats(client, &stats);

printf("Data Channels:\n");
printf("  Reliable: %lld bytes sent, %lld dropped\n",
       stats.reliable_sent_bytes, stats.reliable_dropped);
printf("  Lossy: %lld bytes sent, %lld dropped\n",
       stats.lossy_sent_bytes, stats.lossy_dropped);
```

**Interpreting drops:**
- **Reliable drops**: Should be zero (indicates serious connection issues)
- **Lossy drops**: Normal under high load or poor network
- **High drops**: Reduce send rate or use smaller packets

---

## Connection Management

### Connection States

The connection lifecycle includes these states:

```cpp
typedef enum {
    LkConnConnecting = 0,     // Initial connection attempt
    LkConnConnected = 1,      // Successfully connected
    LkConnReconnecting = 2,   // Temporary disconnect, attempting reconnect
    LkConnDisconnected = 3,   // Cleanly disconnected
    LkConnFailed = 4          // Connection permanently failed
} LkConnectionState;
```

### Monitoring Connection State

```cpp
void on_connection(void* user, LkConnectionState state,
                   int32_t reason_code, const char* message) {
    switch (state) {
        case LkConnConnecting:
            printf("Connecting to server...\n");
            show_loading_ui();
            break;
            
        case LkConnConnected:
            printf("Connected successfully!\n");
            hide_loading_ui();
            enable_features();
            break;
            
        case LkConnReconnecting:
            printf("Connection lost, reconnecting...\n");
            show_reconnecting_ui();
            pause_features();
            break;
            
        case LkConnDisconnected:
            printf("Disconnected: %s\n", message ? message : "normal");
            cleanup_session();
            break;
            
        case LkConnFailed:
            fprintf(stderr, "Connection failed (code %d): %s\n",
                   reason_code, message ? message : "unknown");
            show_error_ui(message);
            cleanup_session();
            break;
    }
}

lk_set_connection_callback(client, on_connection, user_data);
```

### Reconnection Configuration

Configure automatic reconnection behavior:

```cpp
// Set reconnection backoff parameters
lk_set_reconnect_backoff(
    client,
    100,    // Initial backoff (ms)
    5000,   // Maximum backoff (ms)
    1.5     // Backoff multiplier
);
```

The SDK will automatically attempt reconnection with exponential backoff.

### Manual Reconnection

```cpp
// Detect failed connection
void on_connection(void* user, LkConnectionState state,
                   int32_t code, const char* msg) {
    if (state == LkConnFailed) {
        // Wait before manual retry
        sleep(5);
        
        // Try to reconnect
        LkResult result = lk_connect(client, url, new_token);
        if (result.code != 0) {
            // Handle failure
        }
    }
}
```

### Token Refresh

**Note**: Runtime token refresh is not currently supported by the SDK. Use disconnect/reconnect:

```cpp
LkResult result = lk_refresh_token(client, new_token);
if (result.code == 501) {
    // Not supported, use fallback
    lk_disconnect(client);
    lk_connect(client, url, new_token);
}
```

---

## Advanced Features

### Logging Configuration

Control log verbosity:

```cpp
// Set log level
lk_set_log_level(client, LkLogDebug);

// Available levels:
// LkLogError  - Errors only
// LkLogWarn   - Warnings and errors
// LkLogInfo   - Info, warnings, errors (default)
// LkLogDebug  - Debug + above
// LkLogTrace  - All logging (very verbose)
```

### Role Management

Roles determine publishing and subscribing capabilities:

```cpp
typedef enum {
    LkRoleAuto = 0,        // SDK decides based on token
    LkRolePublisher = 1,   // Can publish, won't auto-subscribe
    LkRoleSubscriber = 2,  // Can subscribe, won't publish
    LkRoleBoth = 3         // Can publish and subscribe
} LkRole;

// Connect with specific role
lk_connect_with_role(client, url, token, LkRolePublisher);
```

**Note**: Dynamic role switching requires disconnect/reconnect:

```cpp
LkResult result = lk_set_role(client, LkRoleSubscriber, 1);
if (result.code == 501) {
    // Not supported, reconnect instead
    lk_disconnect(client);
    lk_connect_with_role(client, url, token, LkRoleSubscriber);
}
```

### Statistics Collection

Periodic monitoring example:

```cpp
void monitor_thread() {
    while (running) {
        // Audio stats
        LkAudioStats audio_stats;
        if (lk_get_audio_stats(client, &audio_stats).code == 0) {
            log_metric("audio.underruns", audio_stats.underruns);
            log_metric("audio.overruns", audio_stats.overruns);
            log_metric("audio.buffer_fill", 
                      (float)audio_stats.ring_queued_frames / 
                      audio_stats.ring_capacity_frames);
        }
        
        // Data stats
        LkDataStats data_stats;
        if (lk_get_data_stats(client, &data_stats).code == 0) {
            log_metric("data.reliable_dropped", data_stats.reliable_dropped);
            log_metric("data.lossy_dropped", data_stats.lossy_dropped);
        }
        
        sleep(5);  // Poll every 5 seconds
    }
}
```

---

## Unreal Engine Integration

The repository includes a complete Unreal Engine plugin (`LiveKitBridge`) that wraps the FFI library with Blueprint-friendly components.

### Quick Setup

1. Copy the plugin to your project:
   ```
   YourProject/Plugins/LiveKitBridge/
   ```

2. Enable the plugin in your `.uproject` or via the Plugins window

3. Add `ULiveKitPublisherComponent` to an Actor

4. Configure in Blueprint or C++:
   - **RoomUrl**: `ws://127.0.0.1:7880` (local) or `wss://your-server.livekit.io`
   - **Token**: Your JWT access token
   - **Role**: Publisher, Subscriber, or Both

### Using ULiveKitPublisherComponent

```cpp
// C++ Example
UPROPERTY(VisibleAnywhere)
ULiveKitPublisherComponent* LiveKitComponent;

void AMyActor::BeginPlay() {
    Super::BeginPlay();
    
    LiveKitComponent->RoomUrl = TEXT("wss://server.livekit.io");
    LiveKitComponent->Token = TEXT("your-jwt-token");
    LiveKitComponent->Role = ELiveKitClientRole::Both;
    LiveKitComponent->bReceiveMocap = true;
    
    // Component connects automatically in BeginPlay
}

// Send data from C++
void AMyActor::SendGameState() {
    TArray<uint8> data;
    SerializeGameState(data);
    LiveKitComponent->SendMocap(data, true);  // Reliable
}

// Receive data in Blueprint
// Implement OnMocapReceived event
```

### Blueprint Integration

Available Blueprint events:
- `OnConnected(URL, Role, bRecvMocap, bRecvAudio)`
- `OnDisconnected()`
- `OnAudioPublishReady(SampleRate, Channels)`
- `OnFirstAudioReceived(SampleRate, Channels, FramesPerChannel)`
- `OnMocapReceived(Payload)`
- `OnMocapSent(Bytes, bReliable)`
- `OnMocapSendFailed(Bytes, bReliable, Reason)`

### Audio Publishing in Unreal

```cpp
// Capture audio and publish
void AMyActor::CaptureAndPublishAudio(float DeltaTime) {
    TArray<int16> AudioSamples;
    int32 FramesPerChannel;
    
    // Capture from microphone or generate audio
    CaptureAudioSamples(AudioSamples, FramesPerChannel);
    
    // Publish via component
    LiveKitComponent->PushAudioPCM(AudioSamples, FramesPerChannel);
}
```

### Multiple Audio Tracks

Create separate tracks for different audio sources:

```cpp
// Create microphone track
LiveKitComponent->CreateAudioTrack(
    FName("microphone"),
    48000,  // Sample rate
    1,      // Mono
    1000    // 1 second buffer
);

// Create game audio track
LiveKitComponent->CreateAudioTrack(
    FName("game-audio"),
    48000,
    2,      // Stereo
    1000
);

// Push to specific tracks
LiveKitComponent->PushAudioPCMOnTrack(
    FName("microphone"), 
    MicSamples, 
    FramesPerChannel
);
```

### Custom Data Channels

Register named channels for different data types:

```cpp
// Register channels
LiveKitComponent->RegisterMocapChannel(
    FName("player-position"),
    TEXT("player-pos"),
    false,  // Lossy
    true    // Ordered
);

LiveKitComponent->RegisterMocapChannel(
    FName("game-state"),
    TEXT("game-state"),
    true,   // Reliable
    true
);

// Send on specific channels
TArray<uint8> positionData;
LiveKitComponent->SendMocapOnChannel(FName("player-position"), positionData);
```

For more details, see:
- [Adapting to Other Plugins](ADAPTING_TO_OTHER_PLUGIN.md)
- [FFI API Guide](FFI_API_GUIDE.md)

---

## Best Practices

### 1. Error Handling

Always check return codes and free error messages:

```cpp
// ✅ GOOD
LkResult r = lk_connect(client, url, token);
if (r.code != 0) {
    log_error("Connect failed: %s", r.message);
    lk_free_str((char*)r.message);  // Always free!
    return false;
}

// ❌ BAD
lk_connect(client, url, token);  // Ignoring result!
```

### 2. Callback Design

Keep callbacks fast and non-blocking:

```cpp
// ✅ GOOD: Quick copy and signal
void on_audio(void* user, const int16_t* pcm, size_t frames,
              int32_t ch, int32_t sr) {
    RingBuffer* ring = (RingBuffer*)user;
    ring_buffer_write(ring, pcm, frames * ch);  // Lock-free write
}

// ❌ BAD: Heavy processing in callback
void on_audio(void* user, const int16_t* pcm, size_t frames,
              int32_t ch, int32_t sr) {
    apply_noise_reduction(pcm, frames);  // DON'T DO THIS
    apply_reverb(pcm, frames);           // DON'T DO THIS
    encode_to_mp3(pcm, frames);          // DON'T DO THIS
}
```

### 3. Audio Pacing

Maintain consistent audio publishing intervals:

```cpp
// ✅ GOOD: Consistent 10ms intervals
void audio_loop() {
    const int frames_per_chunk = 480;  // 10ms @ 48kHz
    const int interval_us = 10000;     // 10ms
    
    while (running) {
        auto start = get_time();
        
        int16_t audio[480];
        capture_audio(audio);
        lk_publish_audio_pcm_i16(client, audio, 480, 1, 48000);
        
        auto elapsed = get_time() - start;
        usleep(interval_us - elapsed);
    }
}

// ❌ BAD: Bursty uploads
void audio_loop() {
    while (running) {
        int16_t audio[4800];  // 100ms all at once
        capture_audio(audio);
        lk_publish_audio_pcm_i16(client, audio, 4800, 1, 48000);
        sleep_ms(100);  // Sleeps exactly 100ms, ignoring processing time
    }
}
```

### 4. Data Packet Sizing

Respect size limits for reliability:

```cpp
// ✅ GOOD: Check size before sending
void send_position_update(const Position& pos) {
    uint8_t buffer[256];
    size_t size = serialize_position(buffer, &pos);
    
    if (size <= 1000) {  // Safe for lossy
        lk_send_data(client, buffer, size, LkLossy);
    } else {
        // Too large, split or use reliable
        lk_send_data(client, buffer, size, LkReliable);
    }
}

// ❌ BAD: Blindly sending large lossy packets
void send_position_update(const Position& pos) {
    uint8_t buffer[5000];  // Too large!
    size_t size = serialize_position(buffer, &pos);
    lk_send_data(client, buffer, size, LkLossy);  // Silently sent as reliable data
}
```

### 5. Resource Cleanup

Proper cleanup order:

```cpp
// ✅ GOOD: Proper cleanup sequence
void cleanup() {
    // 1. Disconnect (blocks until callbacks complete)
    lk_disconnect(client);
    
    // 2. Now safe to free user data (no more callbacks)
    free(user_data);
    
    // 3. Destroy client
    lk_client_destroy(client);
}

// ❌ BAD: Freeing data before disconnect
void cleanup() {
    free(user_data);          // Callbacks might still reference this!
    lk_disconnect(client);    // Callbacks might access freed memory!
    lk_client_destroy(client);
}
```

### 6. Token Management

Use appropriate token lifetimes:

```cpp
// ✅ GOOD: Generate fresh tokens
string generate_token_for_session() {
    // Short-lived token (1 hour)
    return mint_token(identity, room, "1h");
}

// Refresh before expiry
void monitor_token_expiry() {
    if (token_expires_soon()) {
        string new_token = generate_token_for_session();
        lk_disconnect(client);
        lk_connect(client, url, new_token.c_str());
    }
}

// ❌ BAD: Long-lived tokens in production
const char* token = mint_token(identity, room, "365d");  // Too long!
```

### 7. Monitoring and Diagnostics

Regularly monitor pipeline health:

```cpp
void monitor_health() {
    static int last_underruns = 0;
    
    LkAudioStats stats;
    lk_get_audio_stats(client, &stats);
    
    // Alert on new underruns
    if (stats.underruns > last_underruns) {
        log_warning("Audio underruns detected: %d", stats.underruns);
        // Take action: increase buffer, check CPU load
    }
    last_underruns = stats.underruns;
    
    // Alert on high buffer usage
    float buffer_fill = (float)stats.ring_queued_frames / 
                       stats.ring_capacity_frames;
    if (buffer_fill > 0.9f) {
        log_warning("Audio buffer nearly full: %.1f%%", buffer_fill * 100);
    }
}
```

---

## Performance Optimization

### Audio Performance

1. **Use appropriate chunk sizes**: 10-20ms chunks balance latency and overhead
2. **Maintain consistent timing**: Avoid jitter in audio publishing
3. **Pre-allocate buffers**: Reduce allocations in hot paths
4. **Monitor ring buffer**: Watch for underruns/overruns

```cpp
// Optimized audio publishing
class AudioPublisher {
    int16_t buffer[960];  // Pre-allocated (20ms @ 48kHz)
    const int chunk_size = 960;
    
public:
    void publish_chunk() {
        // Fill buffer from audio source
        fill_audio_buffer(buffer, chunk_size);
        
        // Publish (no allocations in hot path)
        lk_publish_audio_pcm_i16(client, buffer, chunk_size/2, 2, 48000);
    }
};
```

### Data Channel Performance

1. **Keep lossy packets small**: ≤ 1000 bytes for best delivery
2. **Batch when possible**: Combine small updates into single packets
3. **Use appropriate reliability**: Lossy for frequent, unreliable for rare

```cpp
// Efficient position updates
class PositionStreamer {
    struct Position { float x, y, z; };
    Position buffer[10];  // Batch up to 10 positions
    int count = 0;
    
public:
    void add_position(float x, float y, float z) {
        buffer[count++] = {x, y, z};
        
        if (count >= 10) {
            flush();
        }
    }
    
    void flush() {
        if (count > 0) {
            lk_send_data(client, (uint8_t*)buffer, 
                        count * sizeof(Position), LkLossy);
            count = 0;
        }
    }
};
```

### Memory Management

1. **Reuse buffers**: Avoid frequent allocations
2. **Use stack allocation**: For small, temporary buffers
3. **Free error messages**: Always call `lk_free_str()`

```cpp
// ✅ Efficient memory usage
class DataSender {
    uint8_t send_buffer[1024];  // Reusable buffer
    
public:
    void send_message(const Message& msg) {
        size_t size = serialize_to_buffer(send_buffer, msg);
        lk_send_data(client, send_buffer, size, LkReliable);
        // No allocation, no free
    }
};
```

### Threading Optimization

1. **Dedicate threads**: Separate threads for audio, data, and monitoring
2. **Use lock-free queues**: For cross-thread communication
3. **Avoid blocking**: Never block in callbacks

```cpp
// Optimized threading model
class OptimizedClient {
    LkClientHandle* client;
    LockFreeQueue<AudioFrame> audio_queue;
    LockFreeQueue<DataPacket> data_queue;
    
    // Dedicated audio thread
    void audio_thread() {
        while (running) {
            AudioFrame frame;
            if (audio_queue.try_pop(frame)) {
                lk_publish_audio_pcm_i16(client, frame.data, 
                                        frame.frames, 1, 48000);
            }
            precise_sleep_us(10000);  // 10ms
        }
    }
    
    // Dedicated data thread
    void data_thread() {
        while (running) {
            DataPacket packet;
            if (data_queue.try_pop(packet)) {
                lk_send_data(client, packet.data, packet.size, packet.reliability);
            }
            sleep_us(1000);  // 1ms
        }
    }
    
    // Callback just enqueues (non-blocking)
    static void on_audio_cb(void* user, const int16_t* pcm, 
                           size_t frames, int32_t ch, int32_t sr) {
        OptimizedClient* self = (OptimizedClient*)user;
        AudioFrame frame(pcm, frames, ch, sr);  // Quick copy
        self->received_queue.push(frame);  // Lock-free push
    }
};
```

### Network Optimization

1. **Configure encoder bitrate**: Balance quality and bandwidth
2. **Monitor data drops**: Reduce send rate if drops increase
3. **Use appropriate reliability**: Don't overuse reliable channel

```cpp
// Adaptive data rate
class AdaptiveStreamer {
    int target_fps = 30;
    
public:
    void update_based_on_stats() {
        LkDataStats stats;
        lk_get_data_stats(client, &stats);
        
        float drop_rate = (float)stats.lossy_dropped / 
                         (stats.lossy_sent_bytes + 1);
        
        if (drop_rate > 0.1f) {  // More than 10% drops
            target_fps = max(10, target_fps - 5);
            log_info("Reducing send rate to %d FPS", target_fps);
        } else if (drop_rate < 0.01f) {  // Very low drops
            target_fps = min(60, target_fps + 5);
        }
    }
};
```

---

## Further Reading

- **[FFI API Reference](FFI_API_GUIDE.md)** - Detailed API documentation
- **[Local Server Setup](LOCAL_LIVEKIT_QUICKSTART.md)** - Run LiveKit locally
- **[Token Generation](TOKEN_MINTING.md)** - Create access tokens
- **[Plugin Integration](ADAPTING_TO_OTHER_PLUGIN.md)** - Integrate into other engines
- **[Architecture Overview](ARCHITECTURE.md)** - System design and internals
- **[Examples](EXAMPLES.md)** - Complete working examples
- **[Troubleshooting](TROUBLESHOOTING.md)** - Common issues and solutions

---

## Support

- **Issues**: [GitHub Issues](https://github.com/lifelike-and-believable/livekit-ffi-ue/issues)
- **LiveKit Docs**: [docs.livekit.io](https://docs.livekit.io/)
- **License**: MIT
