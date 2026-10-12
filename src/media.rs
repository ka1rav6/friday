//! Cross-platform local media: screen capture, audio recording, speech
//! synthesis, OCR and offline speech recognition.
//!
//! Linux is the primary target; macOS and Windows are supported through their
//! own backends. The design is one table of candidate backends per capability,
//! ordered by preference and gated by platform, so adding a backend never
//! touches the tool code.
//!
//! Two safety rules hold everywhere:
//!
//! 1. **No shell.** Binaries are spawned with argument arrays, chosen from a
//!    compile-time candidate list — never from model-supplied text.
//! 2. **Parameters never enter script text.** The Windows backends run a
//!    *constant* PowerShell script and receive their file path or text through
//!    environment variables, so nothing user- or model-supplied is ever parsed
//!    as code.
//!
//! Every entry point reports precisely which dependency is missing and the
//! install command for the current OS, rather than failing obscurely.
use anyhow::{bail, Context, Result};
use base64::Engine;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::process::Command;

// ------------------------------------------------------------------ platform

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Linux,
    MacOs,
    Windows,
    Other,
}

pub fn os() -> Os {
    if cfg!(target_os = "linux") {
        Os::Linux
    } else if cfg!(target_os = "macos") {
        Os::MacOs
    } else if cfg!(target_os = "windows") {
        Os::Windows
    } else {
        Os::Other
    }
}

impl Os {
    pub fn as_str(&self) -> &'static str {
        match self {
            Os::Linux => "linux",
            Os::MacOs => "macos",
            Os::Windows => "windows",
            Os::Other => "other",
        }
    }
}

/// On Linux, whether this is a Wayland or X11 session — they need different
/// screenshot tools, and picking the wrong one fails confusingly.
pub fn display_server() -> Option<&'static str> {
    if os() != Os::Linux {
        return None;
    }
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        Some("wayland")
    } else if std::env::var_os("DISPLAY").is_some() {
        Some("x11")
    } else {
        None
    }
}

// ------------------------------------------------------------------ spawning

/// Resolves a bare program name against `PATH`, honouring `PATHEXT` on Windows.
/// Returns `None` rather than guessing, so a missing dependency is always
/// reported as missing.
pub fn which(program: &str) -> Option<PathBuf> {
    // Candidate names are compile-time constants and always bare; reject
    // anything path-like so a location can never be smuggled in.
    if program.is_empty() || program.contains('/') || program.contains('\\') {
        return None;
    }
    let sep = if cfg!(windows) { ';' } else { ':' };
    let exts: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into())
            .split(';')
            .filter(|e| !e.is_empty())
            .map(|e| e.to_ascii_lowercase())
            .collect()
    } else {
        vec![String::new()]
    };

    let dirs = std::env::var_os("PATH")?;
    for dir in dirs.to_str()?.split(sep).filter(|d| !d.is_empty()) {
        let base = Path::new(dir).join(program);
        if base.is_file() {
            return Some(base);
        }
        for ext in &exts {
            if ext.is_empty() {
                continue;
            }
            let with_ext = Path::new(dir).join(format!("{program}{ext}"));
            if with_ext.is_file() {
                return Some(with_ext);
            }
        }
    }
    None
}

pub struct Output {
    pub status: Option<i32>,
    pub success: bool,
    pub stdout: Vec<u8>,
    pub stderr: String,
}

impl std::fmt::Debug for Output {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Output")
            .field("status", &self.status)
            .field("success", &self.success)
            .field("stdout_bytes", &self.stdout.len())
            .field("stderr", &self.stderr)
            .finish()
    }
}

/// Spawns `program` with `args`, bounded by `timeout_secs`, with credentials
/// stripped from the child environment. `env` adds variables for the child
/// only — the mechanism the Windows backends use to pass parameters without
/// putting them in script text.
pub async fn run_with_env(
    program: &str,
    args: &[&str],
    env: &[(&str, &str)],
    timeout_secs: u64,
    stdin: Option<&[u8]>,
) -> Result<Output> {
    let path = which(program).ok_or_else(|| anyhow::anyhow!("{program} is not on PATH"))?;
    let mut cmd = Command::new(path);
    cmd.args(args)
        .env_remove("NEBIUS_API_KEY")
        .env_remove("TAVILY_API_KEY")
        .env_remove("GOOGLE_CLIENT_SECRET")
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for (k, v) in env {
        cmd.env(k, v);
    }

    let mut child = cmd
        .spawn()
        .with_context(|| format!("failed to start {program}"))?;
    if let Some(bytes) = stdin {
        use tokio::io::AsyncWriteExt;
        let mut sink = child.stdin.take().context("stdin was not piped")?;
        sink.write_all(bytes).await?;
        sink.shutdown().await?;
    }
    let out = tokio::time::timeout(
        std::time::Duration::from_secs(timeout_secs),
        child.wait_with_output(),
    )
    .await
    .map_err(|_| anyhow::anyhow!("{program} did not finish within {timeout_secs}s"))??;
    Ok(Output {
        status: out.status.code(),
        success: out.status.success(),
        stdout: out.stdout,
        stderr: crate::util::truncate(
            &crate::util::redact(&String::from_utf8_lossy(&out.stderr)),
            4000,
        ),
    })
}

pub async fn run(
    program: &str,
    args: &[&str],
    timeout_secs: u64,
    stdin: Option<&[u8]>,
) -> Result<Output> {
    run_with_env(program, args, &[], timeout_secs, stdin).await
}

/// PowerShell 7 first, then the Windows-bundled 5.1.
const POWERSHELL: &[&str] = &["pwsh", "powershell"];

fn powershell() -> Option<&'static str> {
    POWERSHELL.iter().copied().find(|p| which(p).is_some())
}

/// Runs a **constant** PowerShell script with parameters supplied through the
/// environment. `script` must never be built from untrusted input.
async fn run_powershell(
    script: &'static str,
    env: &[(&str, &str)],
    timeout_secs: u64,
) -> Result<Output> {
    let shell = powershell().ok_or_else(|| {
        anyhow::anyhow!(
            "neither pwsh nor powershell is on PATH, so the Windows backends cannot run"
        )
    })?;
    run_with_env(
        shell,
        &[
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            script,
        ],
        env,
        timeout_secs,
        None,
    )
    .await
}

// ------------------------------------------------------------------ backends

/// One way of providing a capability.
#[derive(Debug, Clone, Copy)]
pub struct Backend {
    /// Display name: a binary name, or "powershell (System.Drawing)".
    pub name: &'static str,
    /// The binary that must exist. `None` means a built-in OS scripting host.
    pub binary: Option<&'static str>,
    pub platforms: &'static [Os],
    /// What this backend is, in a few words.
    pub note: &'static str,
    /// How to actually install it. Written per backend rather than derived
    /// from a package manager, because the package name frequently differs
    /// from the binary name (tesseract ships as `tesseract-ocr` on Debian) and
    /// some backends have no package at all.
    pub install: &'static str,
}

impl Backend {
    fn usable(&self) -> bool {
        if !self.platforms.contains(&os()) {
            return false;
        }
        match self.binary {
            Some(b) => which(b).is_some(),
            // A built-in backend still needs its scripting host present.
            None => powershell().is_some(),
        }
    }
}

fn pick(backends: &'static [Backend]) -> Option<Backend> {
    backends.iter().copied().find(|b| b.usable())
}

/// Install guidance for a capability with no usable backend on this OS.
fn hint(capability: &str, backends: &'static [Backend]) -> String {
    let here = os();
    let relevant: Vec<&Backend> = backends
        .iter()
        .filter(|b| b.platforms.contains(&here))
        .collect();
    if relevant.is_empty() {
        return format!("{capability} is not supported on {}.", here.as_str());
    }
    let options: Vec<String> = relevant
        .iter()
        .map(|b| format!("{} ({}) — {}", b.name, b.note, b.install))
        .collect();
    format!(
        "{capability} is unavailable on {}: no backend is installed. Options:\n  - {}",
        here.as_str(),
        options.join("\n  - ")
    )
}

const LINUX: &[Os] = &[Os::Linux];
const MAC: &[Os] = &[Os::MacOs];
const WINDOWS: &[Os] = &[Os::Windows];
const UNIXY: &[Os] = &[Os::Linux, Os::MacOs];
const ALL: &[Os] = &[Os::Linux, Os::MacOs, Os::Windows, Os::Other];

// ------------------------------------------------------------------ screen

pub const SCREENSHOT: &[Backend] = &[
    Backend {
        name: "grim", binary: Some("grim"), platforms: LINUX,
        note: "Wayland, wlroots compositors",
        install: "apt install grim",
    },
    Backend {
        name: "spectacle", binary: Some("spectacle"), platforms: LINUX,
        note: "KDE Plasma, Wayland or X11",
        install: "apt install kde-spectacle",
    },
    Backend {
        name: "gnome-screenshot", binary: Some("gnome-screenshot"), platforms: LINUX,
        note: "GNOME, Wayland or X11",
        install: "apt install gnome-screenshot",
    },
    Backend {
        name: "maim", binary: Some("maim"), platforms: LINUX,
        note: "X11",
        install: "apt install maim",
    },
    Backend {
        name: "scrot", binary: Some("scrot"), platforms: LINUX,
        note: "X11",
        install: "apt install scrot",
    },
    Backend {
        name: "import", binary: Some("import"), platforms: LINUX,
        note: "X11, part of ImageMagick",
        install: "apt install imagemagick",
    },
    Backend {
        name: "screencapture", binary: Some("screencapture"), platforms: MAC,
        note: "built into macOS",
        install: "already installed; grant Screen Recording permission in System Settings > Privacy & Security",
    },
    Backend {
        name: "powershell (System.Drawing)", binary: None, platforms: WINDOWS,
        note: "built into Windows",
        install: "already installed; no action needed",
    },
];

/// Captures the whole primary screen as PNG. The path is passed through the
/// environment on Windows so it never becomes part of the script.
const PS_SCREENSHOT: &str = "\
$ErrorActionPreference='Stop';
Add-Type -AssemblyName System.Windows.Forms,System.Drawing;
$b=[System.Windows.Forms.Screen]::PrimaryScreen.Bounds;
$bmp=New-Object System.Drawing.Bitmap $b.Width,$b.Height;
$g=[System.Drawing.Graphics]::FromImage($bmp);
$g.CopyFromScreen($b.Location,[System.Drawing.Point]::Empty,$b.Size);
$bmp.Save($env:FRIDAY_CAPTURE_PATH,[System.Drawing.Imaging.ImageFormat]::Png);
$g.Dispose(); $bmp.Dispose();";

pub fn screenshot_backend() -> Option<Backend> {
    pick(SCREENSHOT)
}

pub fn screenshot_hint() -> String {
    let mut msg = hint("screen capture", SCREENSHOT);
    // Only the screenshot backends are display-server specific, so this note
    // belongs here and nowhere else.
    if let Some(server) = display_server() {
        msg.push_str(&format!(
            "\nThis is a {server} session: pick a {server} backend — the others will fail even once \
             installed."
        ));
    }
    msg
}

/// Captures the screen to `dest` as PNG. Returns the backend used.
pub async fn capture_screen(dest: &Path) -> Result<&'static str> {
    let backend = screenshot_backend().ok_or_else(|| anyhow::anyhow!("{}", screenshot_hint()))?;
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let out = dest.to_string_lossy().to_string();

    let result = match backend.binary {
        Some("grim") | Some("maim") => run(backend.binary.unwrap(), &[&out], 30, None).await?,
        Some("scrot") => run("scrot", &["--overwrite", &out], 30, None).await?,
        Some("import") => run("import", &["-window", "root", &out], 30, None).await?,
        Some("spectacle") => run("spectacle", &["-b", "-n", "-f", "-o", &out], 30, None).await?,
        Some("gnome-screenshot") => run("gnome-screenshot", &["-f", &out], 30, None).await?,
        Some("screencapture") => run("screencapture", &["-x", &out], 30, None).await?,
        None => run_powershell(PS_SCREENSHOT, &[("FRIDAY_CAPTURE_PATH", &out)], 45).await?,
        Some(other) => bail!("unsupported screenshot backend {other}"),
    };
    if !result.success || !dest.is_file() {
        bail!(
            "{} failed (exit {:?}): {}",
            backend.name,
            result.status,
            if result.stderr.is_empty() {
                "no error output. On Wayland, check that the compositor allows screen capture."
                    .to_string()
            } else {
                result.stderr
            }
        );
    }
    Ok(backend.name)
}

// ------------------------------------------------------------------ OCR

pub const OCR: &[Backend] = &[Backend {
    name: "tesseract",
    binary: Some("tesseract"),
    platforms: ALL,
    note: "offline OCR, nothing is uploaded",
    install: "apt install tesseract-ocr · brew install tesseract · winget install UB-Mannheim.TesseractOCR",
}];

pub fn ocr_backend() -> Option<Backend> {
    pick(OCR)
}

pub fn ocr_hint() -> String {
    let mut h = hint("offline OCR", OCR);
    h.push_str(
        "\nAlternatively, set media.vision_model to a vision-capable model id and FRIDAY will read \
         the screen with that instead of OCR (which uploads the image).",
    );
    h
}

/// Extracts text from an image with tesseract, which writes to stdout when the
/// output base is the literal `stdout`.
pub async fn ocr(image: &Path) -> Result<String> {
    ocr_backend().ok_or_else(|| anyhow::anyhow!("{}", ocr_hint()))?;
    let path = image.to_string_lossy().to_string();
    let out = run("tesseract", &[&path, "stdout", "--psm", "3"], 90, None).await?;
    if !out.success {
        bail!("tesseract failed (exit {:?}): {}", out.status, out.stderr);
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

// ------------------------------------------------------------------ audio in

pub const RECORD: &[Backend] = &[
    Backend {
        name: "arecord",
        binary: Some("arecord"),
        platforms: LINUX,
        note: "ALSA",
        install: "apt install alsa-utils",
    },
    Backend {
        name: "ffmpeg",
        binary: Some("ffmpeg"),
        platforms: ALL,
        note: "works on all three platforms",
        install: "apt install ffmpeg · brew install ffmpeg · winget install Gyan.FFmpeg",
    },
    Backend {
        name: "sox",
        binary: Some("rec"),
        platforms: UNIXY,
        note: "provides the `rec` command",
        install: "apt install sox · brew install sox",
    },
];

pub fn record_backend() -> Option<Backend> {
    pick(RECORD)
}

pub fn record_hint() -> String {
    hint("audio recording", RECORD)
}

/// Records `seconds` of mono 16 kHz WAV audio — the format speech recognition
/// expects. Input device selection is per-OS: PulseAudio/ALSA on Linux,
/// AVFoundation on macOS, DirectShow on Windows.
pub async fn record_audio(dest: &Path, seconds: u32) -> Result<&'static str> {
    let backend = record_backend().ok_or_else(|| anyhow::anyhow!("{}", record_hint()))?;
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let out = dest.to_string_lossy().to_string();
    let secs = seconds.to_string();

    let result = match backend.binary {
        Some("arecord") => {
            run(
                "arecord",
                &[
                    "-q", "-f", "S16_LE", "-r", "16000", "-c", "1", "-d", &secs, &out,
                ],
                seconds as u64 + 20,
                None,
            )
            .await?
        }
        Some("rec") => {
            run(
                "rec",
                &["-q", "-r", "16000", "-c", "1", &out, "trim", "0", &secs],
                seconds as u64 + 20,
                None,
            )
            .await?
        }
        Some("ffmpeg") => {
            let (format, device) = match os() {
                Os::Linux => ("pulse", "default"),
                Os::MacOs => ("avfoundation", ":0"),
                Os::Windows => ("dshow", "audio=Microphone"),
                Os::Other => bail!("ffmpeg audio capture is not configured for this platform"),
            };
            run(
                "ffmpeg",
                &[
                    "-hide_banner",
                    "-loglevel",
                    "error",
                    "-y",
                    "-f",
                    format,
                    "-i",
                    device,
                    "-ar",
                    "16000",
                    "-ac",
                    "1",
                    "-t",
                    &secs,
                    &out,
                ],
                seconds as u64 + 25,
                None,
            )
            .await?
        }
        other => bail!("unsupported recorder {other:?}"),
    };

    if !result.success || !dest.is_file() {
        let extra = match os() {
            Os::Windows => {
                " On Windows, `ffmpeg -list_devices true -f dshow -i dummy` lists the \
                             device names; the default guess is \"Microphone\"."
            }
            Os::MacOs => {
                " On macOS, grant microphone access to the terminal in \
                           System Settings > Privacy & Security > Microphone."
            }
            _ => " Check that a microphone is connected and not muted.",
        };
        bail!(
            "{} failed (exit {:?}): {}.{extra}",
            backend.name,
            result.status,
            if result.stderr.is_empty() {
                "no error output".into()
            } else {
                result.stderr
            }
        );
    }
    Ok(backend.name)
}

// ------------------------------------------------------------------ audio out

pub const TTS: &[Backend] = &[
    Backend {
        name: "piper",
        binary: Some("piper"),
        platforms: ALL,
        note: "natural-sounding; needs a downloaded voice model",
        install: "release binaries at github.com/rhasspy/piper",
    },
    Backend {
        name: "espeak-ng",
        binary: Some("espeak-ng"),
        platforms: ALL,
        note: "small and fast; can write WAV files",
        install:
            "apt install espeak-ng · brew install espeak-ng · winget install eSpeak-NG.eSpeak-NG",
    },
    Backend {
        name: "espeak",
        binary: Some("espeak"),
        platforms: UNIXY,
        note: "the older espeak; can write WAV files",
        install: "apt install espeak · brew install espeak",
    },
    Backend {
        name: "spd-say",
        binary: Some("spd-say"),
        platforms: LINUX,
        note: "speech-dispatcher; speaks aloud only, cannot write a file",
        install: "apt install speech-dispatcher",
    },
    Backend {
        name: "pico2wave",
        binary: Some("pico2wave"),
        platforms: LINUX,
        note: "writes WAV files only",
        install: "apt install libttspico-utils",
    },
    Backend {
        name: "say",
        binary: Some("say"),
        platforms: MAC,
        note: "built into macOS",
        install: "already installed; no action needed",
    },
    Backend {
        name: "powershell (System.Speech)",
        binary: None,
        platforms: WINDOWS,
        note: "built into Windows",
        install: "already installed; no action needed",
    },
];

/// Speaks `$env:FRIDAY_TTS_TEXT` aloud. Text arrives through the environment,
/// never inside the script.
const PS_SPEAK: &str = "\
$ErrorActionPreference='Stop';
Add-Type -AssemblyName System.Speech;
$s=New-Object System.Speech.Synthesis.SpeechSynthesizer;
if ($env:FRIDAY_TTS_VOICE) { try { $s.SelectVoice($env:FRIDAY_TTS_VOICE) } catch {} }
$s.Speak($env:FRIDAY_TTS_TEXT); $s.Dispose();";

/// Same, but writes a WAV file instead of playing it.
const PS_SPEAK_FILE: &str = "\
$ErrorActionPreference='Stop';
Add-Type -AssemblyName System.Speech;
$s=New-Object System.Speech.Synthesis.SpeechSynthesizer;
if ($env:FRIDAY_TTS_VOICE) { try { $s.SelectVoice($env:FRIDAY_TTS_VOICE) } catch {} }
$s.SetOutputToWaveFile($env:FRIDAY_TTS_PATH);
$s.Speak($env:FRIDAY_TTS_TEXT); $s.Dispose();";

pub fn tts_backend() -> Option<Backend> {
    pick(TTS)
}

pub fn tts_hint() -> String {
    hint("speech synthesis", TTS)
}

/// Speaks `text`, or writes a WAV to `dest` when one is given. Returns the
/// backend used and whether a file was produced.
pub async fn speak(
    text: &str,
    dest: Option<&Path>,
    voice: Option<&str>,
) -> Result<(&'static str, bool)> {
    let backend = tts_backend().ok_or_else(|| anyhow::anyhow!("{}", tts_hint()))?;
    if let Some(parent) = dest.and_then(|d| d.parent()) {
        std::fs::create_dir_all(parent)?;
    }
    let out_path = dest.map(|d| d.to_string_lossy().to_string());
    let voice = voice.unwrap_or_default();

    // Where a backend can read stdin, the text goes there, so it is never part
    // of an argument vector.
    let result = match (backend.binary, out_path.as_deref()) {
        (Some("piper"), Some(o)) => {
            run("piper", &["--output_file", o], 180, Some(text.as_bytes())).await?
        }
        (Some("piper"), None) => {
            run("piper", &["--output-raw"], 180, Some(text.as_bytes())).await?
        }
        (Some(bin @ ("espeak-ng" | "espeak")), dest_opt) => {
            let mut args: Vec<&str> = vec!["--stdin"];
            if !voice.is_empty() {
                args.extend_from_slice(&["-v", voice]);
            }
            if let Some(o) = dest_opt {
                args.extend_from_slice(&["-w", o]);
            }
            run(bin, &args, 180, Some(text.as_bytes())).await?
        }
        (Some("spd-say"), None) => run("spd-say", &["--wait", text], 180, None).await?,
        (Some("pico2wave"), Some(o)) => run("pico2wave", &["-w", o, text], 180, None).await?,
        (Some("say"), Some(o)) => {
            run(
                "say",
                &["-o", o, "--data-format=LEF32@22050", text],
                180,
                None,
            )
            .await?
        }
        (Some("say"), None) => run("say", &[text], 180, None).await?,
        (None, Some(o)) => {
            run_powershell(
                PS_SPEAK_FILE,
                &[
                    ("FRIDAY_TTS_TEXT", text),
                    ("FRIDAY_TTS_PATH", o),
                    ("FRIDAY_TTS_VOICE", voice),
                ],
                180,
            )
            .await?
        }
        (None, None) => {
            run_powershell(
                PS_SPEAK,
                &[("FRIDAY_TTS_TEXT", text), ("FRIDAY_TTS_VOICE", voice)],
                180,
            )
            .await?
        }
        (Some(other), dest_opt) => bail!(
            "{other} cannot {}. Install one of: {}",
            if dest_opt.is_some() {
                "write a WAV file"
            } else {
                "play audio directly"
            },
            // Only suggest backends that can actually do what was asked, and
            // never suggest the one that just refused.
            TTS.iter()
                .filter(|b| b.platforms.contains(&os()) && b.binary != Some(other))
                .filter(|b| {
                    if dest_opt.is_some() {
                        b.binary != Some("spd-say")
                    } else {
                        b.binary != Some("pico2wave")
                    }
                })
                .map(|b| format!("{} ({})", b.name, b.install))
                .collect::<Vec<_>>()
                .join("; ")
        ),
    };

    if !result.success {
        bail!(
            "{} failed (exit {:?}): {}",
            backend.name,
            result.status,
            result.stderr
        );
    }
    Ok((backend.name, dest.map(|d| d.is_file()).unwrap_or(false)))
}

// ------------------------------------------------------------------ local STT

pub const STT: &[Backend] = &[
    Backend {
        name: "whisper-cli",
        binary: Some("whisper-cli"),
        platforms: ALL,
        note: "whisper.cpp, current binary name",
        install: "brew install whisper-cpp · or build from github.com/ggml-org/whisper.cpp",
    },
    Backend {
        name: "whisper-cpp",
        binary: Some("whisper-cpp"),
        platforms: ALL,
        note: "whisper.cpp as some distributions package it",
        install: "brew install whisper-cpp · or build from github.com/ggml-org/whisper.cpp",
    },
    Backend {
        name: "whisper",
        binary: Some("whisper"),
        platforms: ALL,
        note: "whisper.cpp or openai-whisper on PATH",
        install: "pip install openai-whisper · or build whisper.cpp",
    },
];

pub fn stt_backend() -> Option<Backend> {
    pick(STT)
}

pub fn stt_hint() -> String {
    let mut h = hint("offline speech recognition", STT);
    h.push_str(
        "\nwhisper.cpp also needs a model file: set media.stt_model to its path. \
         Alternatively set NEBIUS_API_KEY and FRIDAY will transcribe through the hosted \
         speech-to-text endpoint instead.",
    );
    h
}

/// Transcribes a local audio file with whisper.cpp, which cannot run without a
/// GGML model file.
pub async fn transcribe_local(audio: &Path, model_path: &str) -> Result<String> {
    let backend = stt_backend().ok_or_else(|| anyhow::anyhow!("{}", stt_hint()))?;
    anyhow::ensure!(
        !model_path.is_empty(),
        "media.stt_model is empty: set it to the path of a whisper.cpp model (.bin) file"
    );
    anyhow::ensure!(
        Path::new(model_path).is_file(),
        "the whisper model {model_path} does not exist"
    );
    let path = audio.to_string_lossy().to_string();
    let bin = backend.binary.unwrap_or("whisper-cli");
    let out = run(
        bin,
        &["-m", model_path, "-f", &path, "-nt", "-np"],
        600,
        None,
    )
    .await?;
    if !out.success {
        bail!("{bin} failed (exit {:?}): {}", out.status, out.stderr);
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

// ------------------------------------------------------------------ helpers

pub fn data_uri(mime: &str, bytes: &[u8]) -> String {
    format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    )
}

pub fn base64_of(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

pub fn mime_for(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("gif") => "image/gif",
        Some("wav") => "audio/wav",
        Some("mp3") => "audio/mpeg",
        Some("m4a" | "mp4") => "audio/mp4",
        Some("ogg" | "opus") => "audio/ogg",
        Some("flac") => "audio/flac",
        Some("webm") => "audio/webm",
        _ => "application/octet-stream",
    }
}

/// What this machine can actually do, so the UI and the agent can say what is
/// possible instead of discovering it on failure.
pub fn capabilities() -> serde_json::Value {
    let describe = |b: Option<Backend>, hint: String| match b {
        Some(b) => serde_json::json!({"available": true, "backend": b.name, "note": b.note}),
        None => serde_json::json!({"available": false, "why_not": hint}),
    };
    serde_json::json!({
        "os": os().as_str(),
        "display_server": display_server(),
        "screenshot": describe(screenshot_backend(), screenshot_hint()),
        "ocr": describe(ocr_backend(), ocr_hint()),
        "audio_record": describe(record_backend(), record_hint()),
        "text_to_speech": describe(tts_backend(), tts_hint()),
        "speech_to_text_local": describe(stt_backend(), stt_hint()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn which_rejects_paths_and_finds_real_binaries() {
        assert!(
            which("/bin/sh").is_none(),
            "an absolute path must not resolve"
        );
        assert!(which("../sh").is_none());
        assert!(
            which("a\\b").is_none(),
            "a Windows-style path must not resolve"
        );
        assert!(which("").is_none());
        assert!(which("definitely-not-a-real-binary-xyz").is_none());
        // Something from the platform's base install must be findable.
        let anchor = if cfg!(windows) { "cmd" } else { "sh" };
        assert!(which(anchor).is_some(), "{anchor} should be on PATH");
    }

    #[test]
    fn every_backend_table_covers_linux_macos_and_windows() {
        for (capability, table) in [
            ("screenshot", SCREENSHOT),
            ("ocr", OCR),
            ("record", RECORD),
            ("tts", TTS),
            ("stt", STT),
        ] {
            for target in [Os::Linux, Os::MacOs, Os::Windows] {
                assert!(
                    table.iter().any(|b| b.platforms.contains(&target)),
                    "{capability} has no backend for {}",
                    target.as_str()
                );
            }
        }
    }

    #[test]
    fn screenshot_and_tts_need_no_install_on_macos_and_windows() {
        // The built-in backends are what make those platforms work out of the box.
        assert!(SCREENSHOT
            .iter()
            .any(|b| b.platforms == MAC && b.binary == Some("screencapture")));
        assert!(SCREENSHOT
            .iter()
            .any(|b| b.platforms == WINDOWS && b.binary.is_none()));
        assert!(TTS
            .iter()
            .any(|b| b.platforms == MAC && b.binary == Some("say")));
        assert!(TTS
            .iter()
            .any(|b| b.platforms == WINDOWS && b.binary.is_none()));
    }

    /// The Windows backends are the one place a parameter could become code.
    /// Their scripts are `&'static str`, so the compiler already guarantees no
    /// runtime interpolation; what this checks is the other half of the
    /// contract — that every input is read from a FRIDAY_-prefixed environment
    /// variable, and nothing else from the environment is trusted.
    #[test]
    fn windows_scripts_read_every_parameter_from_the_environment() {
        for script in [PS_SCREENSHOT, PS_SPEAK, PS_SPEAK_FILE] {
            let refs: Vec<&str> = script
                .match_indices("$env:")
                .map(|(i, _)| {
                    let rest = &script[i + 5..];
                    let end = rest
                        .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                        .unwrap_or(rest.len());
                    &rest[..end]
                })
                .collect();
            assert!(
                !refs.is_empty(),
                "script takes no input from the environment: {script}"
            );
            for name in refs {
                assert!(
                    name.starts_with("FRIDAY_"),
                    "script reads {name:?} from the environment; only FRIDAY_* inputs are intended"
                );
            }
        }
    }

    #[test]
    fn capability_report_explains_every_absence_with_a_fix() {
        let caps = capabilities();
        assert!(caps["os"].is_string());
        for (name, cap) in caps.as_object().unwrap() {
            if !cap.is_object() {
                continue;
            }
            if cap["available"] == serde_json::json!(false) {
                let why = cap["why_not"].as_str().unwrap_or_default();
                assert!(!why.is_empty(), "{name} is unavailable with no explanation");
                assert!(
                    why.contains("install")
                        || why.contains("Install")
                        || why.contains("not supported"),
                    "{name}: the explanation should say how to fix it: {why}"
                );
            } else if cap["available"] == serde_json::json!(true) {
                assert!(cap["backend"].is_string());
            }
        }
    }

    /// Install instructions are written per backend precisely because the
    /// package name often differs from the binary name. This guards the two
    /// cases that were wrong when the hints were generated mechanically.
    #[test]
    fn install_instructions_use_real_package_names() {
        let tesseract = OCR.iter().find(|b| b.binary == Some("tesseract")).unwrap();
        assert!(
            tesseract.install.contains("tesseract-ocr"),
            "the Debian package is tesseract-ocr, not tesseract: {}",
            tesseract.install
        );
        for b in STT {
            assert!(
                !b.install.contains("apt install"),
                "whisper.cpp is not an apt package; {} must not claim it is: {}",
                b.name,
                b.install
            );
        }
        for table in [SCREENSHOT, OCR, RECORD, TTS, STT] {
            for b in table {
                assert!(
                    !b.install.is_empty(),
                    "{} has no install instruction",
                    b.name
                );
            }
        }
    }

    /// The display-server note is only meaningful for screenshots, and was
    /// previously appended to every capability's hint.
    #[test]
    fn the_display_server_note_appears_only_in_the_screenshot_hint() {
        if display_server().is_none() {
            return;
        }
        assert!(
            screenshot_hint().contains("session"),
            "{}",
            screenshot_hint()
        );
        for h in [ocr_hint(), record_hint(), tts_hint(), stt_hint()] {
            assert!(
                !h.contains("session: pick"),
                "a non-display capability mentions the display server: {h}"
            );
        }
    }

    #[test]
    fn mime_detection_covers_what_we_send_to_models() {
        assert_eq!(mime_for(Path::new("a/b.png")), "image/png");
        assert_eq!(mime_for(Path::new("a/b.JPG")), "image/jpeg");
        assert_eq!(mime_for(Path::new("a/b.wav")), "audio/wav");
        assert_eq!(mime_for(Path::new("a/b.xyz")), "application/octet-stream");
    }

    #[test]
    fn data_uri_is_well_formed() {
        assert_eq!(data_uri("image/png", b"hi"), "data:image/png;base64,aGk=");
    }

    #[tokio::test]
    async fn a_missing_binary_is_reported_not_guessed() {
        let e = format!(
            "{:#}",
            run("definitely-not-a-real-binary-xyz", &[], 1, None)
                .await
                .unwrap_err()
        );
        assert!(e.contains("not on PATH"), "{e}");
    }
}
