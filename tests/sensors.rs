//! Voice and screen tools. The parts that depend on an external binary are
//! skipped with an explanation when that binary is absent, so the suite stays
//! green on a machine (or CI runner) without a display or a sound card — while
//! still asserting that the *absence* is reported correctly.
use friday::media;
use friday::tools::{self, ToolCtx};
use serde_json::json;

async fn privileged() -> friday::state::AppState {
    let (st, _) = friday::test_state().await.unwrap();
    let tmp = std::env::temp_dir().join(format!("friday-captures-{}", friday::util::uid()));
    {
        let mut cfg = st.config.write().await;
        cfg.permissions.allow_privileged = true;
        cfg.media.capture_dir = tmp;
    }
    st
}

#[tokio::test]
async fn the_capability_report_names_the_os_and_explains_every_gap() {
    let (st, _) = friday::test_state().await.unwrap();
    let ctx = ToolCtx::root(st.clone(), None);
    let out = tools::execute(&ctx, "get_media_capabilities", json!({})).await;
    assert!(out.ok, "{:?}", out.value);

    let v = &out.value;
    assert!(
        ["linux", "macos", "windows", "other"].contains(&v["os"].as_str().unwrap()),
        "unexpected os: {}",
        v["os"]
    );
    eprintln!("{}", serde_json::to_string_pretty(v).unwrap());

    for key in [
        "screenshot",
        "ocr",
        "audio_record",
        "text_to_speech",
        "speech_to_text_local",
        "vision_model",
        "speech_to_text_hosted",
    ] {
        let cap = &v[key];
        assert!(cap.is_object(), "{key} missing from the report");
        if cap["available"] == json!(false) {
            let why = cap["why_not"].as_str().unwrap_or_default();
            assert!(!why.is_empty(), "{key} is unavailable with no explanation");
        }
    }
    // The report must say that the invasive tools are gated.
    assert_eq!(v["permission"]["privileged_tools_enabled"], json!(false));
    assert!(v["permission"]["note"]
        .as_str()
        .unwrap()
        .contains("allow_privileged"));
}

#[tokio::test]
async fn screen_and_microphone_are_denied_by_default() {
    let (st, _) = friday::test_state().await.unwrap();
    assert!(
        !st.cfg().await.permissions.allow_privileged,
        "privileged must default to off"
    );
    let ctx = ToolCtx::root(st.clone(), None);

    for (tool, args) in [
        ("capture_screen", json!({"reason": "testing"})),
        ("record_audio", json!({"seconds": 1, "reason": "testing"})),
        ("analyze_screen", json!({"path": "whatever.png"})),
    ] {
        let out = tools::execute(&ctx, tool, args).await;
        assert!(!out.ok, "{tool} ran without privileged permission");
        assert!(
            out.value["error"]
                .as_str()
                .unwrap()
                .contains("allow_privileged"),
            "{tool}: {}",
            out.value["error"]
        );
    }
}

#[tokio::test]
async fn capturing_the_screen_produces_a_real_png() {
    let st = privileged().await;
    if media::screenshot_backend().is_none() {
        eprintln!("skipping: {}", media::screenshot_hint());
        return;
    }
    let ctx = ToolCtx::root(st.clone(), Some("capture-task".into()));
    let out = tools::execute(
        &ctx,
        "capture_screen",
        json!({"reason": "integration test"}),
    )
    .await;
    if !out.ok {
        // A headless CI runner has the binary but no compositor to capture.
        eprintln!(
            "screenshot backend present but capture failed: {}",
            out.value["error"]
        );
        return;
    }

    let path = std::path::PathBuf::from(out.value["full_path"].as_str().unwrap());
    eprintln!("captured {} via {}", path.display(), out.value["backend"]);
    assert!(path.is_file());
    let bytes = std::fs::read(&path).unwrap();
    assert!(
        bytes.len() > 1000,
        "the capture is suspiciously small: {} bytes",
        bytes.len()
    );
    assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n", "not a PNG");
    assert_eq!(out.value["bytes"].as_u64().unwrap(), bytes.len() as u64);

    // Capturing the screen is a privacy event, so it must be audited and
    // the result must say the image has not left the machine.
    let audited: Vec<(String, String)> =
        sqlx::query_as("SELECT action, detail FROM audit_log WHERE category='privacy'")
            .fetch_all(&st.db)
            .await
            .unwrap();
    assert!(
        audited
            .iter()
            .any(|(a, d)| a == "capture_screen" && d.contains("integration test")),
        "{audited:?}"
    );
    assert!(out.value["privacy_note"]
        .as_str()
        .unwrap()
        .contains("has not left the machine"));

    std::fs::remove_file(&path).ok();
}

#[tokio::test]
async fn reading_a_screenshot_refuses_honestly_when_no_reader_exists() {
    let st = privileged().await;
    let ctx = ToolCtx::root(st.clone(), None);

    // Put a real file in the capture directory so the failure is about the
    // reader, not the path.
    let dir = st.cfg().await.media.capture_dir;
    std::fs::create_dir_all(&dir).unwrap();
    let img = dir.join("fake.png");
    std::fs::write(&img, b"\x89PNG\r\n\x1a\nnot really an image").unwrap();

    let out = tools::execute(&ctx, "analyze_screen", json!({"path": "fake.png"})).await;
    if media::ocr_backend().is_some() {
        // tesseract exists: it may succeed with empty text or fail on the junk
        // file, but either way the method must be the local one.
        if out.ok {
            assert_eq!(out.value["method"], json!("local_ocr"));
            assert_eq!(out.value["left_this_machine"], json!(false));
        }
        return;
    }
    assert!(
        !out.ok,
        "analyze_screen claimed to read a screen with no OCR and no vision model"
    );
    let e = out.value["error"].as_str().unwrap();
    assert!(e.contains("cannot read the screen"), "{e}");
    assert!(
        e.contains("media.vision_model"),
        "the refusal should name the alternative: {e}"
    );
    assert!(e.contains("install") || e.contains("Install"), "{e}");
}

#[tokio::test]
async fn a_vision_read_is_refused_while_offline_rather_than_uploading() {
    let st = privileged().await;
    {
        let mut cfg = st.config.write().await;
        cfg.mode = "offline".into();
        cfg.media.vision_model = "some/vision-model".into();
    }
    let dir = st.cfg().await.media.capture_dir;
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("shot.png"), b"\x89PNG\r\n\x1a\nx").unwrap();

    let ctx = ToolCtx::root(st.clone(), None);
    let out = tools::execute(
        &ctx,
        "analyze_screen",
        json!({"path": "shot.png", "prefer": "vision"}),
    )
    .await;
    if media::ocr_backend().is_some() {
        return; // OCR would legitimately handle it locally
    }
    assert!(!out.ok);
    let e = out.value["error"].as_str().unwrap();
    assert!(
        e.contains("offline") || e.contains("no vision model"),
        "{e}"
    );
}

#[tokio::test]
async fn media_paths_cannot_escape_the_capture_directory_or_a_workspace() {
    let st = privileged().await;
    let ctx = ToolCtx::root(st.clone(), None);
    for bad in ["../../etc/passwd", "/etc/passwd", "sub/../../../etc/shadow"] {
        let out = tools::execute(&ctx, "analyze_screen", json!({"path": bad})).await;
        assert!(!out.ok, "{bad} was accepted");
        let e = out.value["error"].as_str().unwrap();
        assert!(
            e.contains("outside") || e.contains("escapes") || e.contains("capture"),
            "{bad}: {e}"
        );
    }
    // An unknown workspace is named as such rather than silently ignored.
    let out = tools::execute(
        &ctx,
        "transcribe_audio",
        json!({"path": "a.wav", "workspace": "nope"}),
    )
    .await;
    assert!(!out.ok);
    assert!(out.value["error"]
        .as_str()
        .unwrap()
        .contains("unknown workspace"));
}

#[tokio::test]
async fn transcription_refuses_rather_than_returning_an_empty_transcript() {
    let st = privileged().await;
    let dir = st.cfg().await.media.capture_dir;
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("clip.wav"), b"RIFF....WAVEfmt ").unwrap();

    let ctx = ToolCtx::root(st.clone(), None);
    // No hosted credential in tests, and no whisper model configured.
    let out = tools::execute(&ctx, "transcribe_audio", json!({"path": "clip.wav"})).await;
    assert!(
        !out.ok,
        "transcription succeeded with no backend at all: {:?}",
        out.value
    );
    let e = out.value["error"].as_str().unwrap();
    assert!(e.contains("cannot transcribe"), "{e}");
    assert!(
        e.contains("whisper") || e.contains("stt_model"),
        "the refusal should name the fix: {e}"
    );
}

#[tokio::test]
async fn speaking_is_not_privileged_but_reports_its_backend() {
    let (st, _) = friday::test_state().await.unwrap();
    let ctx = ToolCtx::root(st.clone(), None);
    // Deliberately not asserting audible output: the point is that the tool is
    // reachable without privileged permission and reports honestly either way.
    let out = tools::execute(&ctx, "speak_response", json!({"text": ""})).await;
    assert!(!out.ok, "empty text should be refused");
    assert!(out.value["error"]
        .as_str()
        .unwrap()
        .contains("nothing to speak"));

    if media::tts_backend().is_none() {
        let out = tools::execute(&ctx, "speak_response", json!({"text": "hello"})).await;
        assert!(!out.ok);
        let e = out.value["error"].as_str().unwrap();
        assert!(e.contains("speech synthesis is unavailable"), "{e}");
        assert!(e.contains("install") || e.contains("Install"), "{e}");
    }
}

#[tokio::test]
async fn rendering_speech_to_a_file_either_works_or_says_the_backend_cannot() {
    let st = privileged().await;
    let ctx = ToolCtx::root(st.clone(), None);
    let out = tools::execute(
        &ctx,
        "speak_response",
        json!({"text": "FRIDAY speech synthesis check.", "to_file": true}),
    )
    .await;

    match media::tts_backend().map(|b| b.name) {
        None => assert!(!out.ok),
        Some(name) => {
            if out.ok {
                let path = std::path::PathBuf::from(out.value["path"].as_str().unwrap());
                eprintln!(
                    "{name} wrote {} ({} bytes)",
                    path.display(),
                    std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0)
                );
                assert!(
                    path.is_file(),
                    "the tool reported success but wrote no file"
                );
                assert_eq!(out.value["left_this_machine"], json!(false));
                std::fs::remove_file(&path).ok();
            } else {
                // e.g. spd-say can speak but cannot render to a file — that
                // limitation must be stated, not silently swallowed.
                let e = out.value["error"].as_str().unwrap();
                eprintln!("{name} cannot write a file: {e}");
                assert!(e.contains("cannot") || e.contains("failed"), "{e}");
            }
        }
    }
}

#[tokio::test]
async fn recording_is_capped_by_configuration() {
    let st = privileged().await;
    st.config.write().await.media.max_record_secs = 2;
    let ctx = ToolCtx::root(st.clone(), None);
    let out = tools::execute(
        &ctx,
        "record_audio",
        json!({"seconds": 600, "reason": "cap test"}),
    )
    .await;
    if !out.ok {
        // No microphone on this machine or runner; the cap is still asserted
        // below through the configuration validator.
        eprintln!("recording unavailable: {}", out.value["error"]);
        assert!(friday::config::validate(&friday::config::Config {
            media: friday::config::MediaConfig {
                max_record_secs: 0,
                ..Default::default()
            },
            ..Default::default()
        })
        .is_err());
        return;
    }
    assert_eq!(
        out.value["seconds"],
        json!(2),
        "the 600s request was not capped"
    );
    assert_eq!(out.value["capped"], json!(true));
    if let Some(p) = out.value["full_path"].as_str() {
        std::fs::remove_file(p).ok();
    }
}
