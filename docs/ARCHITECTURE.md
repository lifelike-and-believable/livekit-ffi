# LiveKit FFI - Architecture Overview

This document provides a detailed overview of the LiveKit FFI library's internal architecture, design decisions, and implementation details.

## Table of Contents

1. [System Overview](#system-overview)
2. [Architecture Layers](#architecture-layers)
3. [Threading Model](#threading-model)
4. [Audio Pipeline](#audio-pipeline)
5. [Data Channel Architecture](#data-channel-architecture)
6. [Connection Management](#connection-management)
7. [Memory Management](#memory-management)
8. [Build System](#build-system)
9. [Feature Flags](#feature-flags)
10. [Design Decisions](#design-decisions)

---

## System Overview

The LiveKit FFI library is a bridge between game engines (primarily Unreal Engine) and the LiveKit real-time communication platform. It provides a C API that wraps the LiveKit Rust SDK, enabling seamless integration into applications that use C, C++, or foreign function interfaces.

### Architecture Goals

1. **Simplicity**: Clean C API that's easy to understand and use
2. **Performance**: Minimal overhead, optimized for real-time applications
3. **Safety**: Thread-safe operations with clear ownership semantics
4. **Flexibility**: Support both synchronous and asynchronous patterns
5. **Portability**: Cross-platform support (Windows, macOS, Linux)

### Technology Stack

```
┌─────────────────────────────────────────┐
│      Application (UE, C++, etc.)        │
├─────────────────────────────────────────┤
│         C FFI Layer (livekit_ffi)       │
│  ┌───────────────────────────────────┐  │
│  │   Rust Implementation             │  │
│  │  ┌──────────┐    ┌──────────┐    │  │
│  │  │ Backend  │    │  Stub    │    │  │
│  │  │ LiveKit  │    │ Backend  │    │  │
│  │  └──────────┘    └──────────┘    │  │
│  └───────────────────────────────────┘  │
├─────────────────────────────────────────┤
│         LiveKit Rust SDK                │
│  (livekit, livekit-api, livekit-protocol)│
├─────────────────────────────────────────┤
│      WebRTC (via libwebrtc_sys)         │
└─────────────────────────────────────────┘
```

---

## Architecture Layers

### Layer 1: C API Surface

The top layer exposes a clean C API defined in `livekit_ffi.h`:

```c
// Core types
typedef struct LkClientHandle LkClientHandle;
typedef struct { int32_t code; const char* message; } LkResult;

// Functions
LkClientHandle* lk_client_create(void);
void lk_client_destroy(LkClientHandle*);
LkResult lk_connect(LkClientHandle*, const char* url, const char* token);
```

**Design principles:**
- All handles are opaque pointers
- All functions return `LkResult` for error handling
- All strings are UTF-8 null-terminated
- All memory allocated by FFI must be freed via `lk_free_str()`

### Layer 2: FFI Translation Layer

Implemented in `src/lib.rs`, this layer:
- Translates C types to Rust types
- Manages handle lifetimes
- Converts errors to `LkResult`
- Ensures thread safety

```rust
#[no_mangle]
pub unsafe extern "C" fn lk_client_create() -> *mut LkClientHandle {
    let client = Box::new(LiveKitClient::new());
    Box::into_raw(client) as *mut LkClientHandle
}

#[no_mangle]
pub unsafe extern "C" fn lk_connect(
    handle: *mut LkClientHandle,
    url: *const c_char,
    token: *const c_char,
) -> LkResult {
    // Validate pointers
    if handle.is_null() || url.is_null() || token.is_null() {
        return LkResult::error(101, "Invalid parameters");
    }
    
    // Convert C strings to Rust
    let url = CStr::from_ptr(url).to_str().unwrap();
    let token = CStr::from_ptr(token).to_str().unwrap();
    
    // Call backend
    let client = &mut *(handle as *mut LiveKitClient);
    match client.connect(url, token) {
        Ok(_) => LkResult::ok(),
        Err(e) => LkResult::error(102, &e.to_string()),
    }
}
```

### Layer 3: Backend Abstraction

Two backend implementations:

**1. LiveKit Backend (`backend_livekit.rs`)**
- Uses real LiveKit Rust SDK
- Enabled with `with_livekit` feature flag
- Full WebRTC functionality

**2. Stub Backend (`backend_stub.rs`)**
- No-op implementation for testing
- Always compiled, used when `with_livekit` is not enabled
- Useful for validating build toolchains

```rust
// Backend trait (conceptual)
trait Backend {
    fn connect(&mut self, url: &str, token: &str) -> Result<()>;
    fn publish_audio(&mut self, pcm: &[i16], frames: usize, ch: i32, sr: i32) -> Result<()>;
    fn send_data(&mut self, bytes: &[u8], reliable: bool) -> Result<()>;
}
```

### Layer 4: LiveKit SDK Integration

When `with_livekit` is enabled:
- Uses `livekit` crate for WebRTC operations
- Uses `livekit-api` for room management
- Uses `livekit-protocol` for signaling
- Uses `tokio` for async runtime
- Uses `rtrb` for lock-free audio ring buffers

---

## Threading Model

The FFI library uses a multi-threaded architecture to handle concurrent operations safely.

### Thread Roles

```
┌──────────────────────────────────────────────────────┐
│                  Application Threads                  │
│  ┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈  │
│  │ Audio Thread  │  Data Thread  │  Main Thread  │  │
│  └───────┬───────┴───────┬───────┴───────┬───────┘  │
│          │               │               │           │
│          │ publish_audio │ send_data     │ connect   │
│          ▼               ▼               ▼           │
├──────────────────────────────────────────────────────┤
│              FFI Layer (Thread-Safe)                 │
│  ┌────────────────────────────────────────────────┐ │
│  │         Mutex<ClientState>                     │ │
│  │  ┌──────────────────────────────────────────┐ │ │
│  │  │ Audio Ring Buffer (Lock-Free)            │ │ │
│  │  │ Data Channel Sender (MPSC)               │ │ │
│  │  │ Connection State (Atomic)                │ │ │
│  │  └──────────────────────────────────────────┘ │ │
│  └────────────────────────────────────────────────┘ │
├──────────────────────────────────────────────────────┤
│           LiveKit SDK Internal Threads               │
│  ┌──────────────┐  ┌──────────────┐  ┌───────────┐ │
│  │ Tokio Runtime│  │ WebRTC Thread│  │ Signaling │ │
│  └──────────────┘  └──────────────┘  └───────────┘ │
└──────────────────────────────────────────────────────┘
          │               │               │
          ▼               ▼               ▼
    on_audio()      on_data()      on_connection()
          │               │               │
          └───────────────┴───────────────┘
                         │
                         ▼
                Application Callbacks
```

### Synchronization Primitives

1. **Mutex**: Protects client state
2. **Atomic**: For connection state and flags
3. **Lock-Free Ring Buffer (rtrb)**: For audio data flow
4. **MPSC Channels**: For data channel messages
5. **Arc**: For shared ownership of client state

### Thread Safety Guarantees

- **All API functions are thread-safe**: Can be called from any thread
- **Callbacks may run on background threads**: Application must handle this
- **No blocking in callbacks**: Callbacks should return quickly
- **Shutdown guarantees**: After disconnect/destroy, no callbacks fire

---

## Audio Pipeline

The audio pipeline handles both publishing and subscribing with minimal latency.

### Publishing Pipeline

```
Application
    │
    │ lk_publish_audio_pcm_i16()
    ▼
┌──────────────────┐
│  FFI Validation  │
│  - Check params  │
│  - Verify ready  │
└────────┬─────────┘
         │
         ▼
┌──────────────────┐
│  Format Convert  │
│  - Resample      │
│  - Channel mix   │
└────────┬─────────┘
         │
         ▼
┌──────────────────┐
│   Ring Buffer    │  ← Lock-free write
│  (rtrb::RingBuf) │
└────────┬─────────┘
         │
         │ Background thread reads
         ▼
┌──────────────────┐
│  Audio Encoder   │
│  (Opus codec)    │
└────────┬─────────┘
         │
         ▼
┌──────────────────┐
│  RTP Packetizer  │
└────────┬─────────┘
         │
         ▼
┌──────────────────┐
│  WebRTC Send     │
└──────────────────┘
```

### Key Components

**1. Ring Buffer**
- Lock-free SPSC (Single Producer, Single Consumer)
- Implemented using `rtrb` crate
- Typical size: 2400 frames (50ms @ 48kHz)
- Metrics: underruns, overruns, fill level

**2. Audio Source Adapter**
```rust
struct NativeAudioSource {
    ring: rtrb::Producer<i16>,
    sample_rate: u32,
    channels: u16,
}

impl AudioSource for NativeAudioSource {
    fn capture_frame(&mut self) -> AudioFrame {
        // Read from ring buffer
        let mut samples = vec![0i16; self.chunk_size];
        match self.ring.read_chunk(&mut samples) {
            Ok(n) => AudioFrame::new(samples[..n]),
            Err(_) => {
                // Underrun: return silence
                self.stats.underruns += 1;
                AudioFrame::silence(self.chunk_size)
            }
        }
    }
}
```

**3. Format Conversion**
- Resampling: Uses LiveKit SDK's resampler
- Channel mixing: Mono → Stereo or Stereo → Mono
- Format: Always i16 PCM

### Subscribing Pipeline

```
WebRTC Receive
    │
    ▼
┌──────────────────┐
│  RTP Depacketize │
└────────┬─────────┘
         │
         ▼
┌──────────────────┐
│  Audio Decoder   │
│  (Opus codec)    │
└────────┬─────────┘
         │
         ▼
┌──────────────────┐
│  Format Convert  │
│  - Resample to   │
│    output format │
│  - Mix channels  │
└────────┬─────────┘
         │
         ▼
┌──────────────────┐
│  Audio Callback  │
│  on_audio(pcm)   │
└──────────────────┘
         │
         ▼
    Application
```

### Audio Configuration

Applications can configure:
- **Output format**: Sample rate and channel count for received audio
- **Publish options**: Bitrate, DTX, stereo/mono
- **Buffer size**: Ring buffer capacity (compile-time)

---

## Data Channel Architecture

Data channels support both reliable (TCP-like) and lossy (UDP-like) transmission.

### Data Send Path

```
Application
    │
    │ lk_send_data() / lk_send_data_ex()
    ▼
┌──────────────────┐
│  FFI Validation  │
│  - Size limits   │
│  - Label check   │
└────────┬─────────┘
         │
         ▼
┌──────────────────┐
│  Data Formatter  │
│  - Add metadata  │
│  - Serialize     │
└────────┬─────────┘
         │
         ▼
┌──────────────────┐
│  Channel Router  │
│  - Select DC     │
│  - Reliable/Lossy│
└────────┬─────────┘
         │
         ▼
┌──────────────────┐
│  Data Channel    │
│  (WebRTC SCTP)   │
└────────┬─────────┘
         │
         ▼
    Network
```

### Data Receive Path

```
Network
    │
    ▼
┌──────────────────┐
│  Data Channel    │
│  (WebRTC SCTP)   │
└────────┬─────────┘
         │
         ▼
┌──────────────────┐
│  Parse Metadata  │
│  - Extract label │
│  - Reliability   │
└────────┬─────────┘
         │
         ▼
┌──────────────────┐
│  Data Callback   │
│  on_data(bytes)  │
│  or              │
│  on_data_ex()    │
└──────────────────┘
         │
         ▼
    Application
```

### Channel Management

**Reliable Channel:**
- Built on SCTP (in WebRTC)
- Guaranteed delivery and ordering
- Max recommended size: ~15 KiB
- Error code 202 if exceeded

**Lossy Channel:**
- Used for `LkLossy` only when the sender opts in with `lk_set_lossy_unreliable`;
  by default `LkLossy` data goes out as a reliable byte stream
- Built on SCTP with partial reliability (unordered, no retransmits)
- No delivery or ordering guarantees
- Max recommended size: ~1300 bytes (to fit in single packet); larger
  `LkLossy` payloads are sent as reliable data instead

### Data Channel State

```rust
struct DataChannelState {
    reliable_label: String,
    lossy_label: String,
    reliable_sent_bytes: AtomicU64,
    reliable_dropped: AtomicU64,
    lossy_sent_bytes: AtomicU64,
    lossy_dropped: AtomicU64,
}
```

---

## Connection Management

Connection lifecycle is managed through a state machine.

### Connection States

```
┌──────────────┐
│  Disconnected│◄─────┐
└──────┬───────┘      │
       │ connect()    │
       ▼              │
┌──────────────┐      │
│  Connecting  │      │
└──────┬───────┘      │
       │              │
       ├─── Success ──┤
       │              │
       ▼              │
┌──────────────┐      │
│  Connected   │      │
└──────┬───────┘      │
       │              │
       ├── Timeout ───┤
       │              │
       ▼              │
┌──────────────┐      │
│ Reconnecting │      │
└──────┬───────┘      │
       │              │
       ├─ Fail/Close ─┘
       │
       ▼
┌──────────────┐
│   Failed     │
└──────────────┘
```

### Connection Implementation

```rust
struct ConnectionState {
    state: AtomicU8,  // LkConnectionState enum
    url: Mutex<String>,
    token: Mutex<String>,
    room: Mutex<Option<Room>>,  // LiveKit SDK Room
    callback: Mutex<Option<ConnectionCallback>>,
}

impl ConnectionState {
    fn transition(&self, new_state: LkConnectionState, reason: i32, msg: &str) {
        let old_state = self.state.swap(new_state as u8, Ordering::SeqCst);
        
        // Fire callback
        if let Some(cb) = self.callback.lock().unwrap().as_ref() {
            cb.invoke(new_state, reason, msg);
        }
        
        // Handle state-specific logic
        match new_state {
            LkConnectionState::Connected => {
                // Enable publishing/subscribing
            }
            LkConnectionState::Reconnecting => {
                // Start reconnection timer
            }
            LkConnectionState::Failed => {
                // Clean up resources
            }
            _ => {}
        }
    }
}
```

### Reconnection Logic

```rust
impl ReconnectionManager {
    fn on_disconnect(&mut self, reason: DisconnectReason) {
        if should_reconnect(reason) {
            let backoff = self.calculate_backoff();
            
            tokio::spawn(async move {
                tokio::time::sleep(backoff).await;
                self.attempt_reconnect().await;
            });
        } else {
            self.transition_to_failed(reason);
        }
    }
    
    fn calculate_backoff(&self) -> Duration {
        let attempt = self.reconnect_count;
        let initial = Duration::from_millis(self.initial_backoff_ms);
        let max = Duration::from_millis(self.max_backoff_ms);
        
        let backoff = initial * self.multiplier.powi(attempt as i32);
        backoff.min(max)
    }
}
```

---

## Memory Management

The FFI layer carefully manages memory ownership across the C/Rust boundary.

### Ownership Rules

1. **Handles are owned by application**
   - Created by `lk_client_create()`
   - Destroyed by `lk_client_destroy()`
   - Application must not use handle after destroy

2. **Strings returned from FFI are owned by application**
   - Must be freed with `lk_free_str()`
   - Only applies to strings in `LkResult.message`

3. **Strings passed to FFI are borrowed**
   - FFI makes copies if needed
   - Application can free after function returns

4. **Callback data is borrowed**
   - PCM audio, data bytes are only valid during callback
   - Application must copy if needed beyond callback

### Memory Allocation Patterns

**Handle Creation:**
```rust
#[no_mangle]
pub unsafe extern "C" fn lk_client_create() -> *mut LkClientHandle {
    let client = Box::new(LiveKitClient::new());
    Box::into_raw(client) as *mut LkClientHandle
    // Transfers ownership to C side
}
```

**Handle Destruction:**
```rust
#[no_mangle]
pub unsafe extern "C" fn lk_client_destroy(handle: *mut LkClientHandle) {
    if handle.is_null() {
        return;
    }
    
    // Take back ownership
    let client = Box::from_raw(handle as *mut LiveKitClient);
    // Automatic cleanup when Box is dropped
}
```

**String Allocation:**
```rust
fn create_error_string(msg: &str) -> *const c_char {
    let c_string = CString::new(msg).unwrap();
    c_string.into_raw() as *const c_char
    // Caller must free with lk_free_str()
}

#[no_mangle]
pub unsafe extern "C" fn lk_free_str(ptr: *mut c_char) {
    if ptr.is_null() {
        return;
    }
    
    let _ = CString::from_raw(ptr);
    // Automatic deallocation when CString is dropped
}
```

---

## Build System

The library uses Cargo for building with conditional compilation for features.

### Build Configuration

**Cargo.toml:**
```toml
[lib]
crate-type = ["staticlib", "cdylib"]

[features]
with_livekit = [
    "dep:livekit",
    "dep:tokio",
    "dep:rtrb",
    # ...
]

[dependencies]
livekit = { version = "=0.7.24", optional = true }
tokio = { version = "1.39", optional = true, features = ["rt-multi-thread"] }
rtrb = { version = "0.3", optional = true }
```

### Build Modes

**1. Stub Build (No LiveKit):**
```bash
cargo build --release
# Uses backend_stub.rs
# No WebRTC dependencies
# Fast compilation
```

**2. Full Build (With LiveKit):**
```bash
cargo build --release --features with_livekit
# Uses backend_livekit.rs
# Includes full WebRTC stack
# Requires LLVM/Clang
```

### Conditional Compilation

```rust
// Select backend based on feature flag
#[cfg(feature = "with_livekit")]
mod backend {
    pub use super::backend_livekit::*;
}

#[cfg(not(feature = "with_livekit"))]
mod backend {
    pub use super::backend_stub::*;
}
```

### Platform-Specific Configuration

**Windows:**
- Links against MSVC runtime (`/MD` or `/MT`)
- Generates `.dll`, `.dll.lib`, `.pdb`
- Requires Visual Studio C++ tools

**Linux:**
- Generates `.so` shared library
- May use `lld` for faster linking

**macOS:**
- Generates `.dylib`
- Universal binary support (x86_64 + ARM64)

---

## Feature Flags

### Available Features

1. **with_livekit** (main feature)
   - Enables real LiveKit SDK
   - Required for production use
   - Adds WebRTC dependencies

### Future Feature Flags

Potential future additions:
- `video`: Enable video streaming
- `screen_capture`: Screen sharing support
- `simulcast`: Multi-quality streaming
- `e2ee`: End-to-end encryption

---

## Design Decisions

### Why Rust for FFI Layer?

**Pros:**
- Memory safety without GC
- Zero-cost abstractions
- Excellent FFI support with `extern "C"`
- Strong async/await for LiveKit SDK
- Great dependency management (Cargo)

**Cons:**
- Longer compile times
- Requires Rust toolchain
- Learning curve for contributors

### Why Two-Backend Architecture?

Allows:
- Fast iteration without WebRTC dependencies
- Testing build system without full stack
- Validating FFI layer independently
- Quick builds for documentation/tooling work

### Why Lock-Free Ring Buffer for Audio?

**Requirements:**
- Low latency (< 10ms)
- High throughput (48kHz stereo = 192 KB/s)
- Real-time constraints (no blocking)

**Solution:**
- `rtrb` provides SPSC lock-free buffer
- Minimal CPU overhead
- Predictable performance
- No priority inversion

### Why Separate Audio Tracks?

Allows:
- Multiple audio sources (mic, game audio, etc.)
- Independent volume control
- Selective subscription
- Better organization

### Why Size Limits on Data Channels?

**Lossy limit (1300 bytes):**
- Fits in single IP packet (MTU ~1500)
- Avoids fragmentation
- Better delivery rate
- Lower latency

**Reliable limit (15 KiB):**
- Prevents memory exhaustion
- Ensures reasonable transmission time
- Matches WebRTC buffer limits

### Why Callbacks Instead of Polling?

**Callbacks:**
- Lower latency (immediate notification)
- Better resource usage (no busy polling)
- Simpler API (no complex state management)

**Trade-offs:**
- Threading complexity
- Must be non-blocking
- Harder to debug

---

## Performance Characteristics

### Latency

| Operation | Typical Latency |
|-----------|----------------|
| Audio publish (to ring buffer) | < 1ms |
| Audio receive (callback) | 10-50ms (network) |
| Data send (reliable) | 20-100ms (network) |
| Data send (lossy) | 10-30ms (network) |
| Connection establish | 500-2000ms |

### Throughput

| Stream | Bandwidth |
|--------|-----------|
| Audio (32kbps Opus) | ~4 KB/s |
| Audio (64kbps Opus) | ~8 KB/s |
| Data (lossy @ 30Hz, 1KB) | ~30 KB/s |
| Data (reliable @ 1Hz, 10KB) | ~10 KB/s |

### Memory Usage

| Component | Memory |
|-----------|--------|
| Client handle | ~1-2 MB |
| Audio ring buffer | ~200 KB (50ms @ 48kHz stereo) |
| WebRTC internal | ~10-50 MB |
| **Total per client** | **~20-60 MB** |

---

## Security Considerations

### Token Security

- Tokens are JWTs with expiration
- Tokens grant specific permissions
- Tokens should be short-lived in production
- Never hardcode tokens in client code

### Network Security

- TLS for signaling (`wss://`)
- DTLS-SRTP for media (WebRTC)
- No plaintext transmission

### Memory Safety

- Rust prevents buffer overflows
- No use-after-free bugs
- Thread safety enforced at compile time
- C API uses defensive checks

---

## Future Architecture Improvements

### Planned Enhancements

1. **Video Support**
   - Add video tracks
   - Hardware encoding/decoding
   - Multiple quality layers (simulcast)

2. **Enhanced Diagnostics**
   - Detailed metrics API
   - Performance profiling hooks
   - Network quality estimation

3. **Advanced Audio**
   - Spatial audio support
   - Audio effects (reverb, EQ)
   - Multiple simultaneous tracks

4. **Optimization**
   - SIMD audio processing
   - Zero-copy paths where possible
   - Connection pooling

---

## Contributing to Architecture

When contributing architectural changes:

1. **Document design decisions** in this file
2. **Consider backward compatibility** of C API
3. **Add tests** for new components
4. **Update examples** to demonstrate features
5. **Benchmark** performance-critical paths

---

## References

- **LiveKit SDK**: https://github.com/livekit/rust-sdks
- **WebRTC**: https://webrtc.org/
- **Rust FFI Guide**: https://doc.rust-lang.org/nomicon/ffi.html
- **rtrb (Ring Buffer)**: https://crates.io/crates/rtrb
- **Tokio (Async Runtime)**: https://tokio.rs/

## License

MIT
