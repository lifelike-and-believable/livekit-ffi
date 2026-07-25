# Removing ffmpeg and OpenH264 from `livekit_ffi.dll`

## Why

The shipped `livekit_ffi.dll` links ffmpeg (`libavcodec` / `libavutil`, LGPL)
and OpenH264 (`WelsEnc`, covered by Cisco's binary patent grant). Neither is
reachable: the C ABI in `livekit_ffi/include/livekit_ffi.h` exposes no video
path at all, and the transport is data-channel mocap plus Opus audio.

Unreachable is not a defense — LGPL obligations attach to distributing the
bytes. Because nothing calls into them, removal costs zero functionality.

## Where the codecs come from

`webrtc-sys` does not build libwebrtc. It downloads a prebuilt archive that
LiveKit publishes, produced by `libwebrtc/build_windows.cmd` — a script vendored
inside the `webrtc-sys` crate itself. That script hardcodes the gn args:

```
rtc_use_h264=true ffmpeg_branding="Chrome"
```

That single flag is the entire origin of the ffmpeg and OpenH264 bytes.

This project pins `webrtc-sys 0.3.16` (via `livekit =0.7.24`), whose `.gclient`
targets [`webrtc-sdk/webrtc@m137_release`](https://github.com/webrtc-sdk/webrtc).
Note that `client-sdk-rust`'s `main` is far ahead — `webrtc-sys 0.3.39` on
`m144_release` — so the build scripts must come from the pinned crate, not from
a clone of that repo. `build-libwebrtc.yml` resolves them via `cargo metadata`
for exactly this reason.

In that tree, `modules/video_coding/BUILD.gn` gates only the *dependencies* on
`rtc_use_h264`:

```gn
rtc_library("webrtc_h264") {
  sources = [ "codecs/h264/h264.cc", ... ]   # unconditional
  ...
  if (rtc_use_h264) {
    deps += [ "//third_party/ffmpeg" ]
    deps += [ "//third_party/openh264:encoder" ]
  }
}
```

Setting `rtc_use_h264=false` therefore drops both third-party libraries. It is
the only reference to `//third_party/ffmpeg` in the tree.

## Why no `webrtc-sys` patch is needed

`webrtc-sys` guards its H.264 **encoder** but not its **decoder**:

| File | Line | Guarded? |
| --- | --- | --- |
| `src/video_encoder_factory.cpp` | 30, 52 | yes — `#if defined(WEBRTC_USE_H264)` |
| `src/video_decoder_factory.cpp` | 74–76 | no — calls `webrtc::SupportedH264DecoderCodecs()` |
| `src/video_decoder_factory.cpp` | 113–114 | no — calls `webrtc::H264Decoder::Create()` |

That asymmetry looks like it should break the link with H.264 off. It does not.
Both functions live in `h264.cc`, which is compiled unconditionally (see the
`sources` list above), and each keeps its `#if` *inside* the function body:

```cpp
std::vector<SdpVideoFormat> SupportedH264DecoderCodecs() {
  if (!IsH264CodecSupported())          // false when WEBRTC_USE_H264 is undefined
    return std::vector<SdpVideoFormat>();
  ...
}

std::unique_ptr<H264Decoder> H264Decoder::Create() {
#if defined(WEBRTC_USE_H264)
  ...
#else
  RTC_DCHECK_NOTREACHED();
  return nullptr;
#endif
}
```

So both symbols still exist in `webrtc.lib` and `webrtc-sys` links cleanly. At
runtime `SupportedH264DecoderCodecs()` returns empty, H.264 is never negotiated,
and `H264Decoder::Create()` is never reached — the `RTC_DCHECK_NOTREACHED()`
does not fire.

**No fork or patch of `webrtc-sys` is required.**

The define also propagates correctly. `webrtc-sys-build::webrtc_defines()`
scrapes `-D` flags from the first line of the artifact's `webrtc.ninja`; with
`rtc_use_h264=false`, WebRTC's root `BUILD.gn` never adds `WEBRTC_USE_H264`, so
`webrtc-sys`'s own encoder guard compiles the OpenH264 adapter out.

### On `ffmpeg_branding`

The upstream script also sets `ffmpeg_branding="Chrome"`. Do **not** retarget it
to `"Chromium"` — that arg is declared by `//third_party/ffmpeg`, which leaves
the build graph entirely once `rtc_use_h264=false`, and gn rejects build
arguments it cannot resolve. `build-libwebrtc.yml` removes it instead.

## How to produce a clean build

1. Run the **Build libwebrtc (no H.264)** workflow
   (`.github/workflows/build-libwebrtc.yml`) on the self-hosted Windows runner.
   It stages the build scripts out of the pinned `webrtc-sys` crate, rewrites
   the gn args, builds, and asserts that the resulting `webrtc.ninja` carries no
   `-DWEBRTC_USE_H264` and that `webrtc.lib` has no ffmpeg/OpenH264 markers.

   It is `workflow_dispatch` only and needs roughly 60 GB free plus several
   hours. Never wire it to push or PR — build once per libwebrtc bump.

   Pass a `release_tag` (by convention `libwebrtc-*`) to publish the zip as a
   release asset. Actions artifacts expire after 90 days and need auth to
   fetch, so they cannot be consumed by `build-ffi.yml`; a release asset on
   this public repo is stable and anonymously downloadable.

2. Record the asset in `libwebrtc.lock.json` at the repo root:

   ```json
   {
     "url": "https://github.com/<owner>/<repo>/releases/download/<tag>/libwebrtc-win-x64-release-noh264.zip",
     "sha256": "<digest printed by the Package artifact step>",
     "release_tag": "<tag>",
     "webrtc_sys": "0.3.16",
     "webrtc_milestone": "m137_release",
     "built_from_commit": "<sha>"
   }
   ```

   `build-ffi.yml` downloads that URL, verifies the digest, and points
   `LK_CUSTOM_WEBRTC` at the extracted tree, so `webrtc-sys` skips its prebuilt
   download. Pinning it in-repo rather than in a repository variable keeps the
   DLL reproducible from the repo alone and puts the provenance in the diff.
   With no lockfile, or no `url` in it, the build is unchanged. A `url` with no
   `sha256` is rejected rather than silently trusted.

   `LK_CUSTOM_WEBRTC` must contain `include/`, `lib/webrtc.lib` and
   `webrtc.ninja` at its root.

   Note that publishing a `libwebrtc-*` release deliberately does **not**
   trigger an SDK build in `build-ffi.yml` — otherwise it would attach
   `livekit_ffi` zips, built against the old libwebrtc, to a libwebrtc release.

## Verification

`tools/check-no-video-codecs.ps1` scans a binary for `libavcodec`, `libavutil`,
`avcodec_*`, `av_frame_*`, `openh264`, `WelsEnc`, `WelsDec` and `ISVCEncoder`.
It runs in CI against the release DLL as a **hard failure**, and in
`build-libwebrtc.yml` against `webrtc.lib`. All counts must be zero.

That gate is the point: without it this silently regresses the first time
someone bumps the LiveKit SDK and picks up a new prebuilt libwebrtc. If it ever
fires, the fix is to rebuild libwebrtc for the new `webrtc-sys` and update
`libwebrtc.lock.json` — not to relax the scan.

A symbol scan proves the codecs are gone; it does not prove mocap and Opus audio
still flow. That still needs a functional pass against Unreal and a real LiveKit
room.

## Downstream (Open3DBroadcast)

Once a clean DLL ships:

- Replace the DLL/PDB/lib and update the SHA256 inventory in
  `.../ThirdParty/livekit_ffi/README.md`; record the release tag and commit,
  which the provenance table still lists as TBD.
- Drop the ffmpeg/OpenH264 blocker sections from `THIRD_PARTY_NOTICES.md` and
  `THIRD_PARTY_LICENSES.md`.
- Vendor the libwebrtc `LICENSE.md` that `build_windows.cmd` emits into the
  artifact directory.
