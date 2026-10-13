//! Live acceptance tests against the real desktop and the real Gemini API.
//! Ignored by default; run with:
//!   cargo test live_ -- --ignored --nocapture --test-threads=1
//! They need a graphical session and GEMINI_API_KEY (environment or ../.env).

use std::time::Instant;

use tokio_util::sync::CancellationToken;

use crate::capture::{MonitorHint, capture_monitor};
use crate::configuration::{self, Settings};
use crate::gemini::{self, Endpoint, StreamFailure, Timeouts, Turn, VisualContext, build_request};
use crate::image_processing::{EncodingPreference, ProcessingOptions, process_selection};
use crate::selection::PixelRect;

fn hint() -> MonitorHint {
    #[cfg(target_os = "linux")]
    if crate::platform::is_hyprland() {
        let m = crate::platform::hyprland::focused_monitor().expect("focused monitor");
        return MonitorHint {
            name: Some(m.name),
            physical_position: None,
        };
    }
    MonitorHint::default()
}

fn load_env() {
    let _ = dotenvy::from_filename("../.env");
}

#[test]
#[ignore]
fn live_capture_and_compression_measurements() {
    let cap = capture_monitor(&hint()).expect("capture");
    println!("capture: {:?}", cap.info);
    let (w, h) = cap.image.dimensions();
    assert!(w > 0 && h > 0);
    // Not a blank frame: a real desktop has many distinct colours.
    let mut seen = std::collections::HashSet::new();
    for p in cap.image.pixels().step_by(97) {
        seen.insert(p.0);
    }
    assert!(
        seen.len() > 50,
        "frame looks blank ({} colours)",
        seen.len()
    );

    let regions = [
        (
            "top bar",
            PixelRect {
                x: 0,
                y: 0,
                width: w,
                height: 64.min(h),
            },
        ),
        (
            "left half",
            PixelRect {
                x: 0,
                y: 0,
                width: w / 2,
                height: h,
            },
        ),
        (
            "full screen",
            PixelRect {
                x: 0,
                y: 0,
                width: w,
                height: h,
            },
        ),
        (
            "centre",
            PixelRect {
                x: w / 4,
                y: h / 4,
                width: w / 2,
                height: h / 2,
            },
        ),
    ];
    for (name, rect) in regions {
        for enc in [
            EncodingPreference::Auto,
            EncodingPreference::Png,
            EncodingPreference::Webp,
            EncodingPreference::Jpeg,
        ] {
            let opts = ProcessingOptions {
                max_dimension: 2048,
                jpeg_quality: 90,
                encoding: enc,
            };
            let out = process_selection(&cap.image, rect, opts).expect("process");
            let s = out.stats;
            println!(
                "{name:12} {enc:?}: {}x{} -> {}x{} {:?} {} -> {} bytes ({}%) in {} ms",
                s.original_width,
                s.original_height,
                s.processed_width,
                s.processed_height,
                s.format,
                s.original_bytes,
                s.processed_bytes,
                s.reduction_percent,
                s.duration_ms
            );
        }
    }
}

#[tokio::test]
#[ignore]
async fn live_gemini_capture_stream_and_follow_up() {
    load_env();
    let key = configuration::resolve_api_key().expect("GEMINI_API_KEY");
    let model = std::env::var("GEMINI_MODEL")
        .ok()
        .filter(|m| configuration::is_valid_model_id(m))
        .unwrap_or_else(|| Settings::default().model);
    let client = gemini::build_client(Timeouts::default()).unwrap();

    let check = gemini::check_model(&client, gemini::API_BASE, &key, &model)
        .await
        .expect("model check");
    println!("model: {check:?}");

    // Capture the real screen and send the top status bar (clock, workspace
    // numbers) - content that only exists in the actual pixels.
    let cap = capture_monitor(&hint()).expect("capture");
    let (w, h) = cap.image.dimensions();
    let rect = PixelRect {
        x: 0,
        y: 0,
        width: w,
        height: 64.min(h),
    };
    let processed =
        process_selection(&cap.image, rect, Settings::default().processing_options()).unwrap();
    println!("sent image: {:?}", processed.stats);
    let visual = VisualContext::new(processed.format.mime_type(), &processed.bytes);
    let endpoint = Endpoint {
        base_url: gemini::API_BASE,
        api_key: &key,
        model: &model,
    };

    let q1 = "This is a strip from the top of my screen. List every piece of text you can read in it, left to right.";
    let started = Instant::now();
    let mut deltas = Vec::new();
    let mut first_delta_ms = None;
    let out = gemini::stream_generate_with_retry(
        &client,
        &endpoint,
        &build_request(&visual, &[], q1),
        Timeouts::default(),
        &CancellationToken::new(),
        |d| {
            first_delta_ms.get_or_insert(started.elapsed().as_millis());
            deltas.push((started.elapsed().as_millis(), d.len()));
        },
    )
    .await
    .expect("first answer");
    println!(
        "answer 1 ({} deltas, first after {:?} ms, total {} ms):\n{}\n",
        deltas.len(),
        first_delta_ms,
        started.elapsed().as_millis(),
        out.text
    );
    println!("delta timeline (ms, bytes): {deltas:?}");
    assert!(!out.text.trim().is_empty());

    let history = vec![Turn {
        question: q1.into(),
        answer: out.text.clone(),
    }];
    let q2 = "Based on the same image: what time does the clock show? Answer with just the time.";
    let mut n2 = 0;
    let out2 = gemini::stream_generate_with_retry(
        &client,
        &endpoint,
        &build_request(&visual, &history, q2),
        Timeouts::default(),
        &CancellationToken::new(),
        |_| n2 += 1,
    )
    .await
    .expect("follow-up");
    println!("answer 2 ({n2} deltas): {}", out2.text);
    assert!(!out2.text.trim().is_empty());

    // Cancellation of a real in-flight request.
    let cancel = CancellationToken::new();
    let c2 = cancel.clone();
    let mut got = 0;
    let r = gemini::stream_generate_with_retry(
        &client,
        &endpoint,
        &build_request(
            &visual,
            &[],
            "Write a detailed 600-word essay about what this screen strip suggests about its user.",
        ),
        Timeouts::default(),
        &cancel,
        |_| {
            got += 1;
            c2.cancel();
        },
    )
    .await;
    assert!(
        matches!(r, Err(StreamFailure::Cancelled { .. })),
        "expected cancellation, got {r:?}"
    );
    println!("cancelled after {got} delta(s)");

    // Invalid model and invalid key produce the right error kinds.
    let bad_model = Endpoint {
        model: "gemini-does-not-exist-123",
        ..endpoint
    };
    let r = gemini::stream_generate(
        &client,
        &bad_model,
        &build_request(&visual, &[], "hi"),
        Timeouts::default(),
        &CancellationToken::new(),
        |_| {},
    )
    .await;
    println!("invalid model -> {r:?}");
    assert!(
        matches!(r, Err(StreamFailure::Error(ref e)) if e.kind == crate::errors::ErrorKind::InvalidModel)
    );
    let bad_key = Endpoint {
        api_key: "AIzaSyInvalidKeyForHaloTests000000000",
        ..endpoint
    };
    let r = gemini::stream_generate(
        &client,
        &bad_key,
        &build_request(&visual, &[], "hi"),
        Timeouts::default(),
        &CancellationToken::new(),
        |_| {},
    )
    .await;
    println!("invalid key -> {r:?}");
    assert!(
        matches!(r, Err(StreamFailure::Error(ref e)) if e.kind == crate::errors::ErrorKind::Authentication)
    );
}
