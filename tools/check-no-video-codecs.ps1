Param(
  # Binary to scan. Defaults to the release DLL produced by `cargo build --release`.
  [string]$Binary = "livekit_ffi/target/release/livekit_ffi.dll",
  # Report counts and exit 0 even when markers are found. Used to record a
  # baseline before the H.264-free libwebrtc lands.
  [switch]$ReportOnly
)

$ErrorActionPreference = "Stop"

function Info($msg) { Write-Host $msg -ForegroundColor Cyan }
function Die($msg)  { Write-Error $msg; exit 1 }

$RepoRoot = Split-Path -Parent $PSCommandPath
$RepoRoot = Split-Path -Parent $RepoRoot  # tools -> repo root

if (-not (Test-Path $Binary)) { $Binary = Join-Path $RepoRoot $Binary }
if (-not (Test-Path $Binary)) { Die "[codec-scan] Binary not found: $Binary (build first)" }

# ffmpeg (libavcodec/libavutil) is LGPL and OpenH264 carries Cisco's binary
# patent grant. The FFI exposes no video path at all -- `include/livekit_ffi.h`
# has no video functions -- so neither is reachable, but LGPL attaches to
# distributing the bytes regardless of reachability. These markers must stay at
# zero once libwebrtc is built with `rtc_use_h264=false`.
$Patterns = [ordered]@{
  "libavcodec"  = "libavcodec"
  "libavutil"   = "libavutil"
  "avcodec_api" = "avcodec_(open2|alloc_context3|send_packet|receive_frame)"
  "av_frame"    = "av_frame_(alloc|free|unref)"
  "openh264"    = "openh264"
  "WelsEnc"     = "WelsEnc"
  "WelsDec"     = "WelsDec"
  "ISVCEncoder" = "ISVCEncoder"
}

# .NET resolves relative paths against the process working directory, which is
# not kept in sync with PowerShell's location. Always hand it a full path.
$Binary = (Resolve-Path -LiteralPath $Binary).ProviderPath

Info "[codec-scan] Scanning: $Binary"
$size = (Get-Item -LiteralPath $Binary).Length
Info ("[codec-scan] Size: {0:N0} bytes" -f $size)

# Read in chunks: webrtc.lib runs to hundreds of MB, and slurping it into a
# UTF-16 string would cost roughly double that in memory. Latin-1 maps every
# byte 1:1 to a char, so nothing is lost to multi-byte decoding. Consecutive
# chunks overlap so a marker straddling a boundary is still caught.
$chunkSize = 8MB
$overlap = 64
$enc = [System.Text.Encoding]::GetEncoding(28591)
$counts = @{}
foreach ($name in $Patterns.Keys) { $counts[$name] = 0 }

$stream = [System.IO.File]::OpenRead($Binary)
try {
  $buffer = New-Object byte[] $chunkSize
  $carry = ""
  while (($read = $stream.Read($buffer, 0, $chunkSize)) -gt 0) {
    $carryLen = $carry.Length
    $text = $carry + $enc.GetString($buffer, 0, $read)
    foreach ($name in $Patterns.Keys) {
      foreach ($m in [regex]::Matches($text, $Patterns[$name], 'IgnoreCase')) {
        # A match ending at or before the end of the carried-over region was
        # already fully visible last round and counted then. Anything ending
        # past it is new -- which is also what catches markers straddling the
        # chunk boundary.
        if (($m.Index + $m.Length) -gt $carryLen) { $counts[$name]++ }
      }
    }
    $carry = if ($text.Length -ge $overlap) { $text.Substring($text.Length - $overlap) } else { $text }
  }
} finally {
  $stream.Dispose()
}

$total = 0
$rows = foreach ($name in $Patterns.Keys) {
  $total += $counts[$name]
  [pscustomobject]@{ Marker = $name; Pattern = $Patterns[$name]; Count = $counts[$name] }
}

$rows | Format-Table -AutoSize | Out-String | Write-Host

if ($total -eq 0) {
  Info "[codec-scan] PASS - no ffmpeg/OpenH264 markers found."
  exit 0
}

if ($ReportOnly) {
  Write-Warning "[codec-scan] $total marker hit(s) found (report-only, not failing)."
  exit 0
}

Die "[codec-scan] FAIL - $total ffmpeg/OpenH264 marker hit(s) found in $Binary"
