//! Actual native GPUI window -> actual Rust helper -> sanitized editor acceptance.
//! Explicit cfg/owned-CI entry only. All data and storage belong to this fixture.
use super::*;
use anyhow::{Result, ensure};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::{
    io::{Read, Write},
    net::TcpListener,
    time::{Duration, Instant},
};
#[repr(C)]
struct NativePoint {
    x: i32,
    y: i32,
}
#[repr(C)]
#[derive(Default, PartialEq, Debug)]
struct NativeRect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}
#[link(name = "user32")]
unsafe extern "system" {
    fn ClientToScreen(hwnd: *mut std::ffi::c_void, point: *mut NativePoint) -> i32;
    fn GetClientRect(hwnd: *mut std::ffi::c_void, rect: *mut NativeRect) -> i32;
    fn GetClipboardSequenceNumber() -> u32;
}
fn native(window: &Window) -> Result<usize> {
    match HasWindowHandle::window_handle(window)?.as_raw() {
        RawWindowHandle::Win32(handle) => Ok(handle.hwnd.get() as usize),
        _ => anyhow::bail!("Acceptance needs actual Windows backend"),
    }
}
async fn until(
    handle: WindowHandle<CaptureWorkspace>,
    cx: &mut gpui::AsyncApp,
    predicate: impl Fn(&CaptureWorkspace) -> bool,
) -> Result<()> {
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        if handle.update(cx, |state, _, _| predicate(state))? {
            return Ok(());
        }
        ensure!(Instant::now() < end, "Actual editor operation deadline");
        cx.background_executor()
            .timer(Duration::from_millis(10))
            .await;
    }
}
async fn registration(
    handle: WindowHandle<CaptureWorkspace>,
    cx: &mut gpui::AsyncApp,
    eligible: bool,
    old_token: Option<u64>,
) -> Result<Option<u64>> {
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        let (is_eligible, retiring, token) = handle.update(cx, |_, window, _| {
            gpui_windows::native_drop_acceptance_window_state(native(window)?)
        })??;
        if is_eligible == eligible
            && !retiring
            && (if eligible {
                token.is_some() && token != old_token
            } else {
                token.is_none()
            })
        {
            return Ok(token);
        }
        ensure!(
            Instant::now() < end,
            "Actual native eligibility/retirement deadline"
        );
        cx.background_executor()
            .timer(Duration::from_millis(10))
            .await;
    }
}
pub fn run(case: &str) -> Result<()> {
    let (case, control) = match case {
        "gpui-control-noop-busy-ack" => ("gpui-busy", Some("omit-busy-admission-ack")),
        "gpui-control-noop-cancel-ack" => ("gpui-cancel", Some("omit-cancel-discard-ack")),
        "gpui-control-source-error" => ("gpui-image", Some("mutate-source-error")),
        "gpui-control-source-none" => ("gpui-image", Some("mutate-source-none")),
        "gpui-control-duplicate-acquire" => ("gpui-image", Some("mutate-duplicate-get")),
        "gpui-control-duplicate-release" => ("gpui-image", Some("mutate-duplicate-release")),
        case => (case, None),
    };
    if let Some(control) = control {
        unsafe {
            std::env::set_var("SCREENSHOTS_DROP_ACCEPTANCE_CONTROL", control);
        }
    }
    let case = case.to_owned();
    gpui_windows::run_native_drop_ui_acceptance(&case.clone(), || run_private(case))
}
fn run_private(case: String) -> Result<()> {
    let source_oracle_control = std::env::var("SCREENSHOTS_DROP_ACCEPTANCE_CONTROL")
        .is_ok_and(|value| value.starts_with("mutate-"));
    let root = std::env::temp_dir().join(format!(
        "screenshots-owned-production-editor-{}",
        std::process::id()
    ));
    std::fs::create_dir(&root)?;
    let source = root.join("Owned synthetic é 🖼.png");
    let image = image::RgbaImage::from_raw(2, 1, vec![11, 23, 37, 0, 43, 59, 71, 128]).unwrap();
    image.save(&source)?;
    let original = std::fs::read(&source)?;
    let mut server = None;
    let url = if case == "gpui-url" {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let address = listener.local_addr()?;
        let body = original.clone();
        server = Some(std::thread::Builder::new().name("owned-one-image-http".into()).stack_size(256 * 1024).spawn(move || -> Result<()> {
            let (mut socket, peer) = listener.accept()?; ensure!(peer.ip().is_loopback(), "Only owned loopback source");
            socket.set_read_timeout(Some(Duration::from_secs(5)))?; socket.set_write_timeout(Some(Duration::from_secs(5)))?;
            let mut request=[0u8;4096]; let mut read=0;
            while read<request.len() {
                let count=socket.read(&mut request[read..])?; ensure!(count>0,"Truncated owned HTTP request"); read+=count;
                if request[..read].windows(4).any(|bytes|bytes==b"\r\n\r\n") {break;}
            }
            ensure!(request[..read].starts_with(b"GET /owned.png ") && request[..read].windows(4).any(|bytes|bytes==b"\r\n\r\n"),"Exact bounded owned image request");
            write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len())?; socket.write_all(&body)?; Ok(())
        })?);
        format!("http://{address}/owned.png")
    } else {
        "https://example.invalid/owned.png".into()
    };
    let mut consumer = if case == "gpui-cancel" {
        Some(gpui_windows::native_drop_acceptance_arm_consumer(
            "body-held",
        ))
    } else if case == "gpui-busy" {
        Some(gpui_windows::native_drop_acceptance_arm_consumer(
            "busy-admission",
        ))
    } else {
        None
    };
    if case == "gpui-cancel" {
        gpui_windows::hold_native_drop_acceptance_delivery();
    }
    let result = Arc::new(Mutex::new(None));
    let reported = result.clone();
    let clipboard = unsafe { GetClipboardSequenceNumber() };
    let platform = gpui_platform::current_platform(false);
    let fonts = caption::CaptionFonts(platform.text_system());
    let frames = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let rendered = frames.clone();
    let input_source = source.clone();
    let selected_case = case.clone();
    gpui::Application::with_platform(platform)
        .with_assets(assets::AppAssets)
        .run(move |cx| {
            cx.set_global(fonts);
            cx.set_quit_mode(gpui::QuitMode::Explicit);
            redaction_editor::bind_keys(cx);
            let window = cx
                .open_kit_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(Bounds {
                            origin: point(px(0.), px(0.)),
                            size: size(px(1060.), px(760.)),
                        })),
                        focus: false,
                        ..app_window_options("Owned native drop acceptance")
                    },
                    |window, cx| {
                        window.on_next_frame(move |window, _| {
                            window.on_next_frame(move |_, _| {
                                rendered.store(true, std::sync::atomic::Ordering::Release)
                            })
                        });
                        let view = cx.new(|cx| CaptureWorkspace::isolated_ui(window, cx));
                        view.update(cx, |state, _| {
                            state.settings_loading = false;
                            state.output_directory = Some(root.clone());
                            state.history_database = None;
                            state.upload_profile_checked = true;
                            if selected_case == "gpui-busy" {
                                state.capturing = true;
                                state.capture_status = "Owned active image job".into();
                            }
                        });
                        view
                    },
                )
                .expect("owned actual GPUI window");
            let reported_timeout = reported.clone();
            cx.spawn(async move |cx| {
                cx.background_executor()
                    .timer(Duration::from_secs(40))
                    .await;
                *reported_timeout.lock().unwrap() = Some(Err(anyhow::anyhow!(
                    "Owned actual GPUI acceptance watchdog"
                )));
                cx.update(|cx| cx.quit());
            })
            .detach();
            cx.spawn(async move |cx| {
                let checked: Result<()> = async {
                    let frame_deadline = Instant::now() + Duration::from_secs(10);
                    while !frames.load(std::sync::atomic::Ordering::Acquire) {
                        ensure!(
                            Instant::now() < frame_deadline,
                            "Owned native frame/layout deadline"
                        );
                        cx.background_executor()
                            .timer(Duration::from_millis(10))
                            .await;
                    }
                    let token = registration(window, cx, true, None).await?;
                    if selected_case == "gpui-hide-show" {
                        let before = window.update(cx, |_, window, _| {
                            let hwnd = native(window)?;
                            let mut rect = NativeRect::default();
                            ensure!(
                                unsafe { GetClientRect(hwnd as _, &mut rect) } != 0,
                                "Owned bounds"
                            );
                            Ok::<_, anyhow::Error>(rect)
                        })??;
                        window.update(cx, |_, window, _| {
                            gpui_windows::native_drop_acceptance_rearm(native(window)?)
                        })??;
                        // WM_SHOWWINDOW coalescing is exercised without any resize or visibility observer.
                        window.update(cx, |_, window, _| {
                            let target = native_window(window).map_err(anyhow::Error::msg)?;
                            target.request_visible(false)?;
                            target.request_visible(true)?;
                            Ok::<_, anyhow::Error>(())
                        })??;
                        registration(window, cx, true, token).await?;
                        let after = window.update(cx, |_, window, _| {
                            let hwnd = native(window)?;
                            let mut rect = NativeRect::default();
                            ensure!(
                                unsafe { GetClientRect(hwnd as _, &mut rect) } != 0,
                                "Owned bounds"
                            );
                            Ok::<_, anyhow::Error>(rect)
                        })??;
                        ensure!(
                            before == after,
                            "Hide/show unexpectedly resized the owned window"
                        );
                    }
                    window.update(cx, |_, window, _| {
                        let hwnd = native(window)?;
                        let mut point = NativePoint { x: 600, y: 350 };
                        ensure!(
                            unsafe { ClientToScreen(hwnd as _, &mut point) } != 0,
                            "Owned final point"
                        );
                        gpui_windows::native_drop_acceptance_input(
                            hwnd,
                            &input_source,
                            &url,
                            point.x,
                            point.y,
                        )
                    })??;
                    let source_result=cx.background_executor().spawn(async{gpui_windows::native_drop_acceptance_drive()}).await;
                    let deferred_source=if source_oracle_control {Some(source_result)} else {source_result?;None};
                    if selected_case == "gpui-cancel" {
                        let held = gpui_windows::native_drop_acceptance_await_consumer(
                            consumer.take().unwrap(),
                            cx.background_executor().timer(Duration::from_secs(5)),
                        )
                        .await?;
                        let discarded =
                            gpui_windows::native_drop_acceptance_arm_consumer("cancel-discard");
                        window.update(cx, |state, _, cx| {
                            cx.cancel_pending_native_drop();
                            state.close_overlays(cx);
                        })?;
                        gpui_windows::release_native_drop_acceptance_delivery();
                        let ack = gpui_windows::native_drop_acceptance_await_consumer(
                            discarded,
                            cx.background_executor().timer(Duration::from_secs(5)),
                        )
                        .await?;
                        ensure!(
                            ack.token == held.token && ack.generation == held.generation,
                            "Canceled foreground discarded a different body"
                        );
                        window.update(cx, |state, _, _| {
                            ensure!(
                                state.editor.is_none() && !state.capturing,
                                "Canceled consumed body opened editor"
                            );
                            Ok::<_, anyhow::Error>(())
                        })??;
                    } else if selected_case == "gpui-busy" {
                        gpui_windows::native_drop_acceptance_await_consumer(
                            consumer.take().unwrap(),
                            cx.background_executor().timer(Duration::from_secs(5)),
                        )
                        .await?;
                        window.update(cx, |state, _, _| {
                            ensure!(
                                state.editor.is_none()
                                    && state.capturing
                                    && state.capture_status == "Owned active image job",
                                "Busy consumed body replaced active work"
                            );
                            state.capturing = false;
                            Ok::<_, anyhow::Error>(())
                        })??;
                    } else {
                        until(window, cx, |state| {
                            state.editor.is_some() && !state.capturing
                        })
                        .await?;
                        window.update(cx, |state, _, cx| {
                            let editor = state.editor.unwrap().read(cx)?;
                            ensure!(
                                editor.source.pixels.as_raw() == &[11, 23, 37, 0, 43, 59, 71, 128],
                                "Actual helper/editor pixels or alpha changed"
                            );
                            ensure!(
                                state.last_capture.is_none() && state.last_saved.is_none(),
                                "Drop created output/history state"
                            );
                            state.close_editor(cx);
                            Ok::<_, anyhow::Error>(())
                        })??;
                        until(window, cx, |state| !state.editor_closing).await?;
                        if source_oracle_control {println!("{{\"gpui_editor_verified_before_source_oracle_failure\":true,\"exact_pixels_alpha\":true,\"editor_closed\":true}}");}
                    }
                    window.update(cx, |state, window, _| {
                        ensure!(
                            state.history_entries.is_empty()
                                && state.last_capture.is_none()
                                && state.last_saved.is_none(),
                            "Acceptance created capture/history/output"
                        );
                        native_window(window)
                            .map_err(anyhow::Error::msg)?
                            .request_visible(false)
                    })??;
                    registration(window, cx, false, None).await?;
                    cx.background_executor()
                        .spawn(async { gpui_windows::native_drop_acceptance_wait_reaped() })
                        .await?;
                    window.update(cx,|_,window,_|window.remove_window())?;
                    if let Some(result)=deferred_source{result?;}
                    Ok(())
                }
                .await;
                *reported.lock().unwrap() = Some(checked);
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
    let outcome = result
        .lock()
        .unwrap()
        .take()
        .ok_or_else(|| anyhow::anyhow!("Actual GPUI acceptance did not report"))?;
    if let Some(server) = server {
        server
            .join()
            .map_err(|_| anyhow::anyhow!("Owned HTTP source panicked"))??;
    }
    ensure!(
        unsafe { GetClipboardSequenceNumber() } == clipboard,
        "Owned private clipboard changed"
    );
    ensure!(
        std::fs::read(&source)? == original,
        "Original synthetic file changed"
    );
    ensure!(
        std::fs::read_dir(source.parent().unwrap())?.count() == 1,
        "Acceptance created an output or history file"
    );
    std::fs::remove_file(&source)?;
    std::fs::remove_dir(source.parent().unwrap())?;
    outcome?;
    println!(
        "{{\"acceptance_case\":\"{case}\",\"actual_gpui_helper_editor\":true,\"exact_pixels_alpha_checked\":{},\"outputs\":0,\"private_clipboard_unchanged\":true}}",
        !matches!(case.as_str(), "gpui-busy" | "gpui-cancel")
    );
    Ok(())
}
