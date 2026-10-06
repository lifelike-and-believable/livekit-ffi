# LiveKit FFI - Troubleshooting Guide

This guide helps you diagnose and resolve common issues when using the LiveKit FFI library.

## Table of Contents

1. [Connection Issues](#connection-issues)
2. [Audio Problems](#audio-problems)
3. [Data Channel Issues](#data-channel-issues)
4. [Build and Compilation](#build-and-compilation)
5. [Unreal Engine Integration](#unreal-engine-integration)
6. [Performance Issues](#performance-issues)
7. [Platform-Specific Problems](#platform-specific-problems)
8. [Debugging Tips](#debugging-tips)

---

## Connection Issues

### Cannot Connect to Server

**Symptoms:**
- `lk_connect()` returns error code 101-199
- Connection callback shows `LkConnFailed`
- Timeout after 30 seconds

**Possible Causes and Solutions:**

#### 1. Invalid URL Format

**Problem:** URL is not in correct format

```cpp
// ❌ WRONG
lk_connect(client, "localhost:7880", token);
lk_connect(client, "http://localhost:7880", token);

// ✅ CORRECT
lk_connect(client, "ws://localhost:7880", token);      // Local, no TLS
lk_connect(client, "wss://server.livekit.io", token);  // Production with TLS
```

**Fix:** Use `ws://` for local servers, `wss://` for production

#### 2. Server Not Running

**Check:**
```bash
# Test if server is reachable
curl http://localhost:7880

# For Docker
docker ps | grep livekit
```

**Fix:** Start the LiveKit server
```bash
docker run --rm -p 7880:7880 -p 7882:7882/udp livekit/livekit-server start --dev
```

#### 3. Invalid or Expired Token

**Problem:** Token is malformed or expired

**Check token:**
```bash
# Decode JWT (without verification)
echo "YOUR_TOKEN" | cut -d'.' -f2 | base64 -d 2>/dev/null | jq
```

**Look for:**
- `exp`: Expiration timestamp (Unix epoch)
- `video`: Must include `roomJoin: true`

**Fix:** Generate a new token
```bash
node tools/token-mint/index.js --identity user1 --room test --ttl 24h
```

#### 4. Firewall Blocking Ports

**Required ports:**
- TCP 7880: Signaling
- TCP 7881: TCP fallback (optional)
- UDP 7882: Media

**Fix (Windows):**
```powershell
# Allow UDP 7882
New-NetFirewallRule -DisplayName "LiveKit UDP" -Direction Inbound -Protocol UDP -LocalPort 7882 -Action Allow
```

#### 5. Identity Collision

**Problem:** Two clients using same identity

**Symptoms:**
- First client gets disconnected when second connects
- "Participant already exists" error

**Fix:** Use unique identity for each client
```cpp
// Generate unique identity
char identity[64];
snprintf(identity, sizeof(identity), "user_%d", rand());
// Use this in token generation
```

### Connection Drops Repeatedly

**Symptoms:**
- `LkConnReconnecting` state frequently
- Connection unstable

**Possible Causes:**

#### 1. Network Instability

**Check:**
```bash
# Test connectivity
ping -c 10 your-server.com

# Check packet loss
mtr your-server.com
```

**Fix:**
- Use wired connection instead of WiFi
- Check for network congestion
- Increase reconnection backoff:
```cpp
lk_set_reconnect_backoff(client, 1000, 30000, 2.0);  // More aggressive backoff
```

#### 2. Token Expiring

**Problem:** Token TTL too short

**Fix:** Generate longer-lived tokens for development
```bash
# 7 days
node tools/token-mint/index.js --identity user1 --room test --ttl 168h
```

#### 3. Server Overloaded

**Check server logs** for resource warnings

**Fix:**
- Scale server resources
- Reduce number of participants
- Lower bitrates

---

## Audio Problems

### No Audio Publishing

**Symptoms:**
- `lk_publish_audio_pcm_i16()` returns error
- No audio visible in LiveKit dashboard

**Possible Causes:**

#### 1. Not Connected

**Check:**
```cpp
if (!lk_client_is_ready(client)) {
    printf("Client not ready for publishing\n");
}
```

**Fix:** Wait for `LkConnConnected` state before publishing

#### 2. Wrong Role

**Problem:** Connected as `LkRoleSubscriber`

**Fix:** Use correct role
```cpp
lk_connect_with_role(client, url, token, LkRolePublisher);
// or LkRoleBoth for pub + sub
```

#### 3. Invalid Audio Format

**Problem:** Unsupported sample rate or channel count

**Supported formats:**
- Sample rates: 8000, 16000, 24000, 48000 Hz (48kHz recommended)
- Channels: 1 (mono) or 2 (stereo)

**Fix:**
```cpp
// ✅ CORRECT
lk_publish_audio_pcm_i16(client, pcm, 480, 1, 48000);  // 10ms mono @ 48kHz

// ❌ WRONG
lk_publish_audio_pcm_i16(client, pcm, 500, 4, 96000);  // Unsupported!
```

#### 4. Token Missing Permissions

**Problem:** Token doesn't grant `canPublish`

**Fix:** Include publish permission in token
```bash
node tools/token-mint/index.js \
  --identity user1 \
  --room test \
  --publish \
  --publishData
```

### Audio Underruns/Overruns

**Symptoms:**
- `lk_get_audio_stats()` shows high underrun/overrun count
- Audio stuttering or glitching

**Diagnosis:**
```cpp
LkAudioStats stats;
lk_get_audio_stats(client, &stats);
printf("Underruns: %d, Overruns: %d\n", stats.underruns, stats.overruns);
```

#### High Underruns

**Cause:** Not publishing audio fast enough

**Fix:**
1. **Increase publish frequency:**
```cpp
// ❌ BAD: 50ms chunks
lk_publish_audio_pcm_i16(client, pcm, 2400, 1, 48000);
sleep_ms(50);

// ✅ GOOD: 10ms chunks
lk_publish_audio_pcm_i16(client, pcm, 480, 1, 48000);
sleep_ms(10);
```

2. **Use dedicated thread:**
```cpp
void* audio_thread(void* arg) {
    LkClientHandle* client = (LkClientHandle*)arg;
    while (running) {
        int16_t buffer[480];
        capture_audio(buffer, 480);
        lk_publish_audio_pcm_i16(client, buffer, 480, 1, 48000);
        precise_sleep_us(10000);
    }
    return NULL;
}
```

#### High Overruns

**Cause:** Publishing too fast or network can't keep up

**Fix:**
1. **Reduce bitrate:**
```cpp
lk_set_audio_publish_options(client, 24000, 1, 0);  // Lower bitrate
```

2. **Ensure proper timing:**
```cpp
// Use precise timing
#include <time.h>

struct timespec ts;
clock_gettime(CLOCK_MONOTONIC, &ts);
long target_ns = ts.tv_nsec + 10000000;  // +10ms
// ... publish audio ...
clock_nanosleep(CLOCK_MONOTONIC, TIMER_ABSTIME, &target_ts, NULL);
```

### No Audio Received

**Symptoms:**
- Audio callback never fires
- No audio from other participants

**Possible Causes:**

#### 1. No Audio Callback Set

**Fix:**
```cpp
void on_audio(void* user, const int16_t* pcm, size_t frames,
              int32_t ch, int32_t sr) {
    printf("Received audio: %zu frames\n", frames);
}

lk_client_set_audio_callback(client, on_audio, NULL);
```

#### 2. Wrong Role

**Problem:** Connected as `LkRolePublisher`

**Fix:**
```cpp
lk_connect_with_role(client, url, token, LkRoleSubscriber);
// or LkRoleBoth
```

#### 3. No Other Publishers

**Check:** Are there other participants publishing audio?

**Test:** Use LiveKit web app to verify others are publishing

#### 4. Token Missing Subscribe Permission

**Fix:**
```bash
node tools/token-mint/index.js \
  --identity user1 \
  --room test \
  --subscribe
```

### Audio Quality Poor

**Symptoms:**
- Robotic/distorted audio
- Excessive latency
- Choppy playback

**Fixes:**

1. **Increase bitrate:**
```cpp
lk_set_audio_publish_options(client, 48000, 1, 0);  // Higher quality
```

2. **Use consistent sample rate:**
```cpp
lk_set_audio_output_format(client, 48000, 1);  // Match publish rate
```

3. **Reduce network jitter:**
- Use wired connection
- Close background apps using bandwidth

---

## Data Channel Issues

### Data Not Sending

**Symptoms:**
- `lk_send_data()` returns error
- Data never arrives at receiver

**Possible Causes:**

#### 1. Size Exceeded

**Error codes:**
- 202: Reliable data too large (> 15 KiB)
- 204: Not sent because the connection is reconnecting
- 205: Send timeout elapsed (`lk_set_send_timeout_ms`); the data may or may not have arrived

(`LkLossy` payloads over 1300 bytes do not fail; they are sent as reliable data.)

**Fix:**
```cpp
size_t data_size = prepare_data(buffer);

// Check size before sending
if (reliability == LkLossy && data_size > 1000) {
    // Split into smaller chunks or use reliable
    fprintf(stderr, "Data too large for lossy channel\n");
} else {
    lk_send_data(client, buffer, data_size, reliability);
}
```

#### 2. Not Connected

**Fix:** Check connection state
```cpp
if (lk_client_is_ready(client)) {
    lk_send_data(client, data, len, LkReliable);
} else {
    printf("Cannot send: not connected\n");
}
```

#### 3. Missing Data Permission

**Problem:** Token doesn't grant `canPublishData`

**Fix:**
```bash
node tools/token-mint/index.js \
  --identity user1 \
  --room test \
  --publishData
```

### High Data Drop Rate

**Symptoms:**
- `lk_get_data_stats()` shows many dropped packets
- Lossy data not arriving

**Diagnosis:**
```cpp
LkDataStats stats;
lk_get_data_stats(client, &stats);

float drop_rate = (float)stats.lossy_dropped / 
                 (stats.lossy_sent_bytes + stats.lossy_dropped);
printf("Lossy drop rate: %.1f%%\n", drop_rate * 100);
```

**Fixes:**

1. **Reduce send rate:**
```cpp
// ❌ BAD: 100 FPS
while (running) {
    send_data(data, size);
    sleep_ms(10);  // 100/sec
}

// ✅ GOOD: 30 FPS
while (running) {
    send_data(data, size);
    sleep_ms(33);  // ~30/sec
}
```

2. **Use smaller packets:**
```cpp
// Keep lossy packets under 1000 bytes
if (size > 1000) {
    // Split or compress
}
```

3. **Use reliable for critical data:**
```cpp
if (is_critical(data)) {
    lk_send_data(client, data, len, LkReliable);
} else {
    lk_send_data(client, data, len, LkLossy);
}
```

---

## Build and Compilation

### Cannot Build with `with_livekit` Feature

**Symptoms:**
- Cargo build fails with linker errors
- Missing libclang/LLVM errors

**Platform-Specific Fixes:**

#### Windows

**Problem:** Missing Visual Studio C++ tools

**Fix:**
1. Install Visual Studio 2019+ with C++ workload
2. Open "x64 Native Tools Command Prompt"
3. Build from that prompt:
```powershell
cd livekit_ffi
cargo build --release --features with_livekit
```

**Problem:** Missing LLVM

**Fix:**
```powershell
# Install LLVM
choco install llvm

# Or download from https://releases.llvm.org/
# Set environment variable
$env:LIBCLANG_PATH="C:\Program Files\LLVM\bin"
```

#### Linux

**Problem:** Missing dependencies

**Fix (Ubuntu/Debian):**
```bash
sudo apt-get update
sudo apt-get install -y \
    build-essential \
    pkg-config \
    libssl-dev \
    libclang-dev \
    clang
```

**Fix (Fedora/RHEL):**
```bash
sudo dnf install -y \
    gcc \
    gcc-c++ \
    openssl-devel \
    clang-devel
```

#### macOS

**Problem:** Missing Xcode tools

**Fix:**
```bash
xcode-select --install
```

### Linker Errors

**Symptoms:**
- "undefined reference" errors
- Missing symbols

**Common causes:**

#### 1. Wrong library linked

**Fix (CMake):**
```cmake
# Windows
target_link_libraries(MyApp livekit_ffi.dll.lib)

# Linux/macOS
target_link_libraries(MyApp livekit_ffi)
```

#### 2. DLL not found at runtime (Windows)

**Fix:**
- Copy DLL to executable directory
- Add DLL directory to PATH
- Use delay-loading:
```cmake
set_target_properties(MyApp PROPERTIES 
    LINK_FLAGS "/DELAYLOAD:livekit_ffi.dll")
```

### Rust Compilation Slow

**Symptoms:**
- Build takes 10+ minutes
- High CPU/memory usage

**Fixes:**

1. **Use stub mode for faster iteration:**
```bash
cargo build --release  # No with_livekit
```

2. **Enable incremental compilation:**
```toml
# .cargo/config.toml
[build]
incremental = true
```

3. **Use faster linker (Linux):**
```toml
# .cargo/config.toml
[target.x86_64-unknown-linux-gnu]
linker = "clang"
rustflags = ["-C", "link-arg=-fuse-ld=lld"]
```

---

## Unreal Engine Integration

### Plugin Not Loading

**Symptoms:**
- Plugin disabled in UE
- "Missing dependencies" error

**Fixes:**

1. **Check plugin descriptor:**
```json
// LiveKitBridge.uplugin
{
  "FileVersion": 3,
  "Version": 1,
  "VersionName": "1.0",
  "Modules": [
    {
      "Name": "LiveKitBridge",
      "Type": "Runtime",
      "LoadingPhase": "Default"
    }
  ]
}
```

2. **Regenerate project files:**
```bash
# Right-click .uproject → "Generate Visual Studio project files"
```

3. **Check module dependencies:**
```csharp
// LiveKitBridge.Build.cs
PublicDependencyModuleNames.AddRange(new string[]
{
    "Core",
    "CoreUObject",
    "Engine",
    "Projects"  // Required for DLL loading
});
```

### DLL Not Loading

**Symptoms:**
- "Failed to load livekit_ffi.dll" in log
- Plugin loads but crashes

**Diagnosis:**
```cpp
// Check UE log (Output Log window)
// Look for:
LogLiveKitBridge: Display: Attempting to load livekit_ffi.dll
LogLiveKitBridge: Error: Failed to load DLL
```

**Fixes:**

1. **Verify DLL location:**
```
YourProject/
  Plugins/LiveKitBridge/
    Binaries/Win64/
      LiveKitBridge.dll  ✓
    ThirdParty/livekit_ffi/
      bin/Win64/Release/
        livekit_ffi.dll  ✓ Should be here
        livekit_ffi.pdb  ✓
```

2. **Check dependencies:**
```powershell
# Use Dependency Walker or dumpbin
dumpbin /dependents livekit_ffi.dll

# Common missing: VCRUNTIME140.dll, MSVCP140.dll
# Install Visual C++ Redistributable
```

3. **Try manual load:**
```cpp
// In module startup
void* Handle = FPlatformProcess::GetDllHandle(TEXT("livekit_ffi.dll"));
if (Handle == nullptr) {
    UE_LOG(LogTemp, Error, TEXT("Failed to load DLL"));
}
```

### Component Not Connecting

**Symptoms:**
- `BeginPlay()` runs but no connection
- No events firing

**Diagnosis:**
```cpp
// Add logging to component
void ULiveKitPublisherComponent::BeginPlay() {
    Super::BeginPlay();
    
    UE_LOG(LogTemp, Display, TEXT("BeginPlay: URL=%s"), *RoomUrl);
    UE_LOG(LogTemp, Display, TEXT("BeginPlay: Token=%s"), *Token);
    
    // Connection code...
}
```

**Fixes:**

1. **Check properties are set:**
```cpp
// In editor, verify:
// - RoomUrl is set (e.g., ws://localhost:7880)
// - Token is set and valid
// - Role is appropriate
```

2. **Use connection callback:**
```cpp
// Implement this in Blueprint
UFUNCTION(BlueprintImplementableEvent)
void OnConnected(const FString& Url, ELiveKitClientRole Role, 
                 bool bRecvMocap, bool bRecvAudio);

// Will be called when connection succeeds
```

### Crashes in Callbacks

**Symptoms:**
- Crash when audio/data received
- Access violation in callback

**Common causes:**

#### 1. Accessing UObject from wrong thread

**Problem:** Callbacks run on background threads

**Fix:**
```cpp
// ❌ WRONG: Direct UObject access from callback
void on_audio(void* user, const int16_t* pcm, size_t frames, ...) {
    UMyComponent* Comp = (UMyComponent*)user;
    Comp->ProcessAudio(pcm, frames);  // CRASH: Not on game thread!
}

// ✅ CORRECT: Queue for game thread
void on_audio(void* user, const int16_t* pcm, size_t frames, ...) {
    UMyComponent* Comp = (UMyComponent*)user;
    
    // Copy data
    TArray<int16> AudioData;
    AudioData.Append(pcm, frames * channels);
    
    // Queue for game thread
    AsyncTask(ENamedThreads::GameThread, [Comp, AudioData]() {
        if (IsValid(Comp)) {
            Comp->ProcessAudio(AudioData);
        }
    });
}
```

#### 2. Component deleted while callback pending

**Fix:** Use weak pointers
```cpp
TWeakObjectPtr<UMyComponent> WeakThis = this;

void on_audio(void* user, const int16_t* pcm, size_t frames, ...) {
    TWeakObjectPtr<UMyComponent>* WeakPtr = 
        (TWeakObjectPtr<UMyComponent>*)user;
    
    AsyncTask(ENamedThreads::GameThread, [WeakPtr, ...]() {
        if (WeakPtr->IsValid()) {
            UMyComponent* Comp = WeakPtr->Get();
            Comp->ProcessAudio(...);
        }
    });
}
```

---

## Performance Issues

### High CPU Usage

**Symptoms:**
- Application using >50% CPU
- Frame drops in game

**Diagnosis:**

1. **Profile with tools:**
```cpp
// Windows: Windows Performance Analyzer
// Linux: perf, flamegraph
// macOS: Instruments
```

2. **Check audio thread:**
```cpp
// Monitor audio stats
LkAudioStats stats;
lk_get_audio_stats(client, &stats);
// High underruns/overruns = CPU struggling
```

**Fixes:**

1. **Reduce audio frequency:**
```cpp
// From 100 FPS to 30 FPS
sleep_ms(33);  // Instead of 10
```

2. **Lower audio bitrate:**
```cpp
lk_set_audio_publish_options(client, 24000, 1, 0);  // From 32k to 24k
```

3. **Disable DTX if causing issues:**
```cpp
lk_set_audio_publish_options(client, 32000, 0, 0);  // DTX off
```

### High Memory Usage

**Symptoms:**
- Application using >1GB RAM
- Memory growing over time

**Diagnosis:**
```cpp
// Check for leaks
// 1. Are you freeing error messages?
LkResult r = lk_send_data(...);
if (r.code != 0) {
    // MUST free this!
    lk_free_str((char*)r.message);
}

// 2. Are you copying data in callbacks?
void on_data(void* user, const uint8_t* bytes, size_t len) {
    // If storing, make sure to free eventually
    uint8_t* copy = malloc(len);
    memcpy(copy, bytes, len);
    // ... must free copy later!
}
```

**Fixes:**

1. **Always free error messages:**
```cpp
LkResult r = lk_connect(client, url, token);
if (r.code != 0) {
    log_error(r.message);
    lk_free_str((char*)r.message);  // IMPORTANT!
}
```

2. **Limit buffering:**
```cpp
// Don't buffer unlimited audio
if (audio_queue.size() > MAX_BUFFERED_FRAMES) {
    // Drop oldest or skip this frame
}
```

### High Bandwidth Usage

**Symptoms:**
- Network usage >1 Mbps per client
- Slow for other applications

**Diagnosis:**
```cpp
LkDataStats stats;
lk_get_data_stats(client, &stats);
printf("Bandwidth: %lld bytes/sec\n", 
       (stats.reliable_sent_bytes + stats.lossy_sent_bytes) / elapsed_sec);
```

**Fixes:**

1. **Lower audio bitrate:**
```cpp
lk_set_audio_publish_options(client, 24000, 1, 0);  // 24 kbps
```

2. **Reduce data send rate:**
```cpp
// From 60 FPS to 20 FPS
sleep_ms(50);  // Instead of 16
```

3. **Compress data:**
```cpp
// Use efficient serialization
// Quantize floats to int16
// Delta encode position updates
```

---

## Platform-Specific Problems

### Windows: Access Violation

**Cause:** Calling convention mismatch

**Fix:** Ensure using `extern "C"` and `__cdecl`:
```c
// Functions should be:
extern "C" __declspec(dllimport) LkClientHandle* lk_client_create(void);
```

### Linux: Library Not Found

**Symptom:**
```
error while loading shared libraries: liblivekit_ffi.so: cannot open shared object file
```

**Fix:**
```bash
# Option 1: Add to LD_LIBRARY_PATH
export LD_LIBRARY_PATH=/path/to/lib:$LD_LIBRARY_PATH

# Option 2: Install system-wide
sudo cp liblivekit_ffi.so /usr/local/lib/
sudo ldconfig

# Option 3: Use RPATH in build
gcc ... -Wl,-rpath,'$ORIGIN/lib'
```

### macOS: Code Signing Issues

**Symptom:**
```
"liblivekit_ffi.dylib" cannot be opened because the developer cannot be verified
```

**Fix:**
```bash
# Remove quarantine attribute
xattr -d com.apple.quarantine liblivekit_ffi.dylib

# Or sign it
codesign -s - --force --deep liblivekit_ffi.dylib
```

---

## Debugging Tips

### Enable Verbose Logging

```cpp
// Set to most verbose level
lk_set_log_level(client, LkLogTrace);
```

### Capture Network Traffic

```bash
# Wireshark filter for LiveKit
udp.port == 7882 or tcp.port == 7880

# tcpdump
sudo tcpdump -i any -n port 7880 or port 7882
```

### Inspect Tokens

```bash
# Decode JWT (3 parts: header.payload.signature)
TOKEN="eyJhbGc..."

# Decode payload
echo $TOKEN | cut -d'.' -f2 | base64 -d 2>/dev/null | jq

# Check expiration
echo $TOKEN | cut -d'.' -f2 | base64 -d 2>/dev/null | jq '.exp' | \
  xargs -I{} date -d @{}
```

### Memory Debugging

**Valgrind (Linux):**
```bash
valgrind --leak-check=full --show-leak-kinds=all ./your_app
```

**AddressSanitizer:**
```bash
# Build with sanitizer
RUSTFLAGS="-Z sanitizer=address" cargo build --release --features with_livekit

# Run
./target/release/your_app
```

### Core Dumps

**Enable core dumps (Linux):**
```bash
ulimit -c unlimited
# Run app, if it crashes:
gdb ./your_app core
(gdb) bt  # Backtrace
```

---

## Getting Help

If you're still stuck:

1. **Check existing issues:** https://github.com/lifelike-and-believable/livekit-ffi-ue/issues

2. **Gather information:**
   - Platform and OS version
   - Build mode (stub or with_livekit)
   - Error messages and logs
   - Minimal reproduction case

3. **Create an issue** with:
   - Clear description of problem
   - Steps to reproduce
   - Expected vs actual behavior
   - Logs and error messages

4. **LiveKit Community:**
   - Slack: https://livekit.io/slack
   - Forum: https://discuss.livekit.io/

---

## Related Documentation

- **[User Guide](USER_GUIDE.md)** - Comprehensive usage guide
- **[Examples](EXAMPLES.md)** - Working code examples
- **[FFI API Reference](FFI_API_GUIDE.md)** - Detailed API docs
- **[Architecture](ARCHITECTURE.md)** - Internal design

## License

MIT
