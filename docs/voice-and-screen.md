# Voice and screen

FRIDAY can take a screenshot, read it, record from the microphone, transcribe
audio, and speak. All of it runs through local backends chosen per platform, so
the same tools work on Linux, macOS and Windows.

Two things are true of every tool here:

- **They are denied by default.** `capture_screen`, `record_audio`,
  `analyze_screen` and `transcribe_audio` are `Permission::Privileged`, which is
  off until you turn it on. A screenshot can contain a password manager; a
  microphone recording can contain anyone in the room.
- **They say whether data left the machine.** Every result carries
  `left_this_machine`, and uploads are audited under the `privacy` category.

## What works where

| Capability | Linux | macOS | Windows |
| --- | --- | --- | --- |
| Screenshot | `grim` (Wayland), `maim`/`scrot`/`import` (X11), `spectacle`, `gnome-screenshot` | `screencapture` — **built in** | PowerShell `System.Drawing` — **built in** |
| Speak | `espeak-ng`, `piper`, `spd-say`, `pico2wave` | `say` — **built in** | PowerShell `System.Speech` — **built in** |
| Record | `arecord`, `ffmpeg`, `sox` | `ffmpeg`, `sox` | `ffmpeg` |
| OCR (offline reading) | `tesseract` | `tesseract` | `tesseract` |
| Offline transcription | whisper.cpp | whisper.cpp | whisper.cpp |

macOS and Windows need **no installation** for screenshots and speech. Linux
needs a screenshot tool matching your display server, which most desktops
already have.

Ask FRIDAY what it has:

```bash
cargo run -- media
```

```
os                     linux
display_server         wayland
screenshot             OK   grim
text_to_speech         OK   spd-say
audio_record           OK   arecord
ocr                    MISS apt install tesseract-ocr · brew install tesseract · winget install UB-Mannheim.TesseractOCR
```

Anything missing is reported with the install command for **your** OS, with the
real package name — `tesseract-ocr` on Debian, not `tesseract`.

## Turning it on

```toml
# .friday/config.toml
[permissions]
allow_privileged = true     # enables screen capture and the microphone

[media]
capture_dir = "data/captures"
prefer_local = true         # use OCR / local whisper before any hosted model
max_capture_bytes = 6291456
max_record_secs = 120
```

## Reading the screen

```
capture_screen  { reason: "the user asked what this error dialog says" }
analyze_screen  { path: "screen-20261012T075103Z.png" }
```

`capture_screen` writes a PNG to `capture_dir` and nothing more — the image has
not left the machine at that point. `analyze_screen` then takes one of two
paths, and tells you which:

| Path | When | `left_this_machine` |
| --- | --- | --- |
| `local_ocr` | tesseract is installed and `prefer_local`, or no vision model, or offline | `false` |
| `vision_model` | `media.vision_model` is set, network is allowed, and local is not preferred | `true` |

If neither is possible it **refuses** and names both fixes. It does not guess at
screen contents.

For the vision path, set a vision-capable model:

```toml
[media]
vision_model = "nvidia/nemotron-3-nano-omni"   # whichever your account lists
```

The image is sent as a base64 data URI in an OpenAI-style `image_url` content
part. Before uploading, FRIDAY checks `max_capture_bytes`, writes an audit entry
naming the model and byte count, and instructs the model not to transcribe
anything that looks like a credential.

A screenshot is **untrusted input**. Text inside an image can say "ignore your
instructions"; the result is wrapped in `<untrusted_content>` and the model is
told to report such text rather than act on it.

## Voice in

```
record_audio     { seconds: 8, reason: "the user wants to dictate a note" }
transcribe_audio { path: "mic-20261012T075210Z.wav" }
```

Recording produces mono 16 kHz WAV — what speech recognition expects — capped
at `max_record_secs` (the result says `capped: true` when your request was
trimmed).

Transcription prefers whichever path keeps the audio local:

| Path | Requires | `left_this_machine` |
| --- | --- | --- |
| `local_whisper` | a whisper.cpp binary **and** `media.stt_model` pointing at a `.bin` model | `false` |
| `hosted` | `NEBIUS_API_KEY`, network allowed, not offline | `true` |

```toml
[media]
stt_model = "/home/you/models/ggml-base.en.bin"   # enables the fully local path
transcribe_model = "whisper-large-v3"             # hosted fallback
```

With neither configured, `transcribe_audio` refuses and names the fix. It never
returns an empty transcript as if it had heard silence.

## Voice out

```
speak_response { text: "Milestone 2 is due Friday." }
speak_response { text: "...", to_file: true }
```

Speech synthesis is local, so `left_this_machine` is always `false`. It is
`LocalMutation`, not privileged — speaking is not invasive. Output is capped at
4000 characters, because a synthesizer reading a long report aloud is not
something you can interrupt from the API.

Backends differ in what they can do, and FRIDAY says so instead of failing
vaguely. `spd-say` can speak but cannot write a WAV, and asking it to produces:

```
spd-say cannot write a WAV file. Install one of: piper (release binaries at
github.com/rhasspy/piper); espeak-ng (apt install espeak-ng · brew install
espeak-ng · winget install eSpeak-NG.eSpeak-NG); espeak (...); pico2wave (...)
```

## How the platform layer is built

One table per capability in [`src/media.rs`](../src/media.rs), ordered by
preference and tagged with the platforms it applies to. Adding a backend is one
table entry; no tool code changes.

Two safety rules hold on every platform:

1. **No shell.** Binaries are spawned with argument arrays, from a compile-time
   candidate list, never from model-supplied text. `which()` refuses anything
   path-like, so a location cannot be smuggled into a candidate name.
2. **Parameters never enter script text.** The Windows backends run a
   `&'static str` PowerShell script and receive the file path or the text to
   speak through *environment variables*. A unit test asserts every `$env:`
   reference in those scripts is a `FRIDAY_`-prefixed variable, so a filename
   can never be parsed as code.

Paths are also confined: `analyze_screen` and `transcribe_audio` accept a file
from `capture_dir` or from an approved workspace, and nothing else. Traversal
attempts are refused.

## Troubleshooting

| Symptom | Fix |
| --- | --- |
| "needs permissions.allow_privileged = true" | Set it in `.friday/config.toml`. |
| Screenshot fails with no error output on Wayland | The compositor is blocking capture. Grant the screen-capture portal permission. |
| macOS screenshot fails | System Settings → Privacy & Security → Screen Recording, enable your terminal. |
| macOS recording fails | Same place, Microphone. |
| Windows recording fails | `ffmpeg -list_devices true -f dshow -i dummy` to find the device name; the default guess is `Microphone`. |
| "offline OCR is unavailable" | Install tesseract, or set `media.vision_model`. |
| Nothing audible but no error | The backend played to a muted sink; try `to_file: true` to confirm synthesis works. |
