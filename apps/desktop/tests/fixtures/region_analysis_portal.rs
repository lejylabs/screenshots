//! Opt-in generated OCR/QR fixture inside capture_portal's private services only.
use super::*;
use image::{Rgba, RgbaImage};

const TEXT: &str = "https://example.invalid/portal-region?literal=$(inert)";

pub(super) fn pixels() -> Result<RgbaImage> {
    let mut pixels = RgbaImage::from_pixel(512, 384, Rgba([255; 4]));
    let glyphs: [[u8; 7]; 9] = [
        [17, 17, 17, 31, 17, 17, 17], // H
        [31, 16, 16, 30, 16, 16, 31], // E
        [16, 16, 16, 16, 16, 16, 31], // L
        [16, 16, 16, 16, 16, 16, 31], // L
        [14, 17, 17, 17, 17, 17, 14], // O
        [0; 7],
        [4, 12, 4, 4, 4, 4, 14],   // 1
        [14, 17, 1, 2, 4, 8, 31],  // 2
        [14, 17, 1, 6, 1, 17, 14], // 3, curved strokes remain distinct from 5
    ];
    for (index, glyph) in glyphs.iter().enumerate() {
        for (row, bits) in glyph.iter().enumerate() {
            for column in 0..5 {
                if bits & (1 << (4 - column)) != 0 {
                    for dy in 0..6 {
                        for dx in 0..6 {
                            pixels.put_pixel(
                                20 + index as u32 * 36 + column * 6 + dx,
                                20 + row as u32 * 6 + dy,
                                Rgba([0, 0, 0, 255]),
                            );
                        }
                    }
                }
            }
        }
    }
    let code = qrcode::QrCode::new(TEXT)?;
    let modules = code.width() as u32;
    let side = (modules + 8) * 4;
    ensure!(
        side <= 224,
        "Private analysis QR fixture exceeds its reserved space"
    );
    for y in 0..side {
        for x in 0..side {
            let (mx, my) = (x / 4, y / 4);
            let dark = mx >= 4
                && my >= 4
                && mx < modules + 4
                && my < modules + 4
                && code[((mx - 4) as usize, (my - 4) as usize)] == qrcode::Color::Dark;
            pixels.put_pixel(
                20 + x,
                150 + y,
                Rgba(if dark { [0, 0, 0, 255] } else { [255; 4] }),
            );
        }
    }
    Ok(pixels)
}

pub(super) fn run_case(root: &Path, address: &str, portal: &Portal) -> Result<()> {
    // This explicit mode requires installed Tesseract+eng; unavailable tools fail the gate.
    let mut reports = Vec::new();
    for source in ["display", "window"] {
        *portal.mode.lock().unwrap() = if source == "window" {
            "v6-window"
        } else {
            "v6"
        }
        .into();
        for (command, area, success) in [
            ("qr-portal-region", ["20", "150", "224", "224"], true),
            ("ocr-portal-region", ["0", "0", "400", "90"], true),
            ("qr-portal-region", ["510", "383", "3", "2"], false),
            ("qr-portal-region", ["400", "0", "100", "100"], false),
        ] {
            let mut process = Command::new(env!("CARGO_BIN_EXE_screenshots"));
            private(&mut process, root, address);
            process.arg(command).arg(source).args(area);
            let result = run(&mut process, Duration::from_secs(45))?;
            ensure!(
                result.status.success() == success,
                "Private {command}/{source} outcome differed: {}",
                String::from_utf8_lossy(&result.stderr)
            );
            if success {
                let report: serde_json::Value = serde_json::from_slice(&result.stdout)?;
                if command == "qr-portal-region" {
                    ensure!(
                        report["codes"]
                            .as_array()
                            .is_some_and(|codes| codes.len() == 1)
                            && report["codes"][0]["text"] == TEXT,
                        "Selected raster QR payload differs"
                    );
                } else {
                    ensure!(
                        report["engine"] == "tesseract"
                            && report["text"].as_str().is_some_and(|text| text
                                .split_whitespace()
                                .collect::<Vec<_>>()
                                .join(" ")
                                == "HELLO 123"),
                        "Selected synthetic raster OCR text differs: {}",
                        report["text"]
                    );
                }
            } else {
                ensure!(
                    result.stdout.is_empty(),
                    "Failed analysis emitted partial JSON"
                );
                ensure!(
                    String::from_utf8_lossy(&result.stderr).contains(if area[0] == "510" {
                        "outside the selected portal raster"
                    } else {
                        "No QR code"
                    }),
                    "Analysis error did not describe the actual rejection"
                );
            }
            ensure!(
                portal.observed.lock().unwrap().sessions.is_empty(),
                "Analysis retained portal session"
            );
            reports.push(
                serde_json::json!({"command":command,"source":source,"expected_success":success}),
            );
        }
    }
    // Permission denial must preserve the same no-output contract as still capture.
    *portal.mode.lock().unwrap() = "denied".into();
    let mut command = Command::new(env!("CARGO_BIN_EXE_screenshots"));
    private(&mut command, root, address);
    command.args(["qr-portal-region", "display", "20", "150", "224", "224"]);
    let result = run(&mut command, Duration::from_secs(25))?;
    ensure!(
        !result.status.success() && result.stdout.is_empty(),
        "Denied analysis published output"
    );
    ensure!(
        portal.observed.lock().unwrap().sessions.is_empty(),
        "Denied analysis retained session"
    );
    // Cancel while waiting for the owned generated source, after permission selection.
    *portal.mode.lock().unwrap() = "v6".into();
    std::fs::write(root.join("stall"), b"private source paused")?;
    let authorized_remotes = portal.observed.lock().unwrap().authorized_remotes;
    if root.join("consumer").exists() {
        std::fs::remove_file(root.join("consumer"))?;
    }
    if root.join("analysis-stalled-stream").exists() {
        std::fs::remove_file(root.join("analysis-stalled-stream"))?;
    }
    let mut command = Command::new(env!("CARGO_BIN_EXE_screenshots"));
    private(&mut command, root, address);
    command.args(["qr-portal-region", "display", "20", "150", "224", "224"]);
    let mut child = ChildGuard(command.spawn()?);
    let deadline = Instant::now() + Duration::from_secs(5);
    while portal.observed.lock().unwrap().authorized_remotes == authorized_remotes
        || !root.join("consumer").is_file()
    {
        ensure!(
            Instant::now() < deadline && child.0.try_wait()?.is_none(),
            "Canceled fixture never obtained its authorized remote and fresh consumer link"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    // Arm only after successful Start/FD handoff and the new source-consumer link.
    std::fs::write(
        root.join("analysis-cancel-armed"),
        b"authorized fresh consumer",
    )?;
    let deadline = Instant::now() + Duration::from_secs(5);
    while !root.join("analysis-stalled-stream").is_file() {
        ensure!(
            Instant::now() < deadline && child.0.try_wait()?.is_none(),
            "Canceled fixture never reached the active stalled stream"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    ensure!(
        unsafe { libc::kill(child.0.id() as i32, libc::SIGTERM) } == 0,
        "Could not signal owned analysis"
    );
    let deadline = Instant::now() + Duration::from_secs(7);
    loop {
        if let Some(status) = child.0.try_wait()? {
            ensure!(!status.success(), "Canceled analysis reported success");
            break;
        }
        ensure!(Instant::now() < deadline, "Canceled analysis did not stop");
        std::thread::sleep(Duration::from_millis(5));
    }
    let mut output = Vec::new();
    std::io::Read::read_to_end(child.0.stdout.as_mut().unwrap(), &mut output)?;
    ensure!(
        output.is_empty(),
        "Canceled analysis published partial JSON"
    );
    let deadline = Instant::now() + Duration::from_secs(3);
    while !portal.observed.lock().unwrap().sessions.is_empty() {
        ensure!(
            Instant::now() < deadline,
            "Canceled analysis retained session"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    std::fs::remove_file(root.join("analysis-cancel-armed"))?;
    std::fs::remove_file(root.join("analysis-stalled-stream"))?;
    std::fs::remove_file(root.join("stall"))?;
    for name in ["HOME", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME"] {
        ensure!(!root.join(name).exists(), "Analysis created user state");
    }
    let observed = portal.observed.lock().unwrap();
    ensure!(
        observed.selected.iter().all(|(_, cursor)| *cursor == 1),
        "Analysis requested a cursor stream"
    );
    ensure!(
        observed.selected.iter().any(|(source, _)| *source == 1)
            && observed.selected.iter().any(|(source, _)| *source == 2),
        "Missing display/window selection"
    );
    let report = serde_json::json!({"cases":reports,"denied_no_output":true,
        "canceled_no_output":true,"canceled_after_authorized_fd_and_stalled_stream":true,"natural_raster":[512,384],"portal_logical_size":[1,1],"session_closed":true,
        "user_state_absent":true,"hardware_acceptance":false,"user_services_used":false});
    if let Some(path) = std::env::var_os("SCREENSHOTS_REGION_ANALYSIS_REPORT") {
        std::fs::write(path, serde_json::to_vec_pretty(&report)?)?;
    }
    eprintln!("{report}");
    Ok(())
}
