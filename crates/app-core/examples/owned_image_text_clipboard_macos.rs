//! Finite owned-child controller; all pasteboard work is on the child native main thread.
#[cfg(target_os = "macos")]
#[allow(dead_code)]
#[path = "../src/clipboard_input_budget.rs"]
mod budget;
#[cfg(target_os = "macos")]
#[path = "../../../apps/desktop/src/file_clipboard_macos.rs"]
mod publisher;
#[cfg(target_os = "macos")]
#[allow(dead_code)]
#[path = "../../../apps/desktop/src/capture_worker_transport.rs"]
mod transport;
#[cfg(target_os = "macos")]
#[allow(dead_code)]
#[path = "../../../apps/desktop/src/capture_worker_unix.rs"]
mod unix;

#[cfg(target_os = "macos")]
struct Native {
    child: std::process::Child,
    group: i32,
    status: Option<std::process::ExitStatus>,
}
#[cfg(target_os = "macos")]
impl Native {
    fn reap(&mut self) -> std::process::ExitStatus {
        if let Some(status) = self.status.as_ref() {
            return status.to_owned();
        }
        // The waitable leader reserves this identity until its whole family is gone.
        loop {
            unix::signal_owned_group(self.group);
            if let Ok(true) = unix::exited_without_reaping(self.group, true) {
                if let Ok(members) = unix::live_members(self.group) {
                    if members.is_empty() {
                        if let Ok(status) = self.child.wait() {
                            self.status = Some(status);
                            return status;
                        }
                    } else if unix::wait_members(self.group, &members) {
                        continue;
                    }
                }
            }
            // Only failed active teardown; never a native pasteboard cleanup call.
            std::thread::park_timeout(std::time::Duration::from_millis(100));
        }
    }
}
#[cfg(target_os = "macos")]
impl Drop for Native {
    fn drop(&mut self) {
        let _ = self.reap();
    }
}

#[cfg(target_os = "macos")]
fn main() -> anyhow::Result<()> {
    use anyhow::{Context, ensure};
    use std::{
        os::unix::process::CommandExt,
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    const CHILD: &str = "__owned-image-text-pasteboard-native";
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    if !args.is_empty() {
        ensure!(
            args.len() == 3 && args[0] == CHILD,
            "Invalid private pasteboard child arguments"
        );
        let parent = args[1]
            .to_str()
            .context("Invalid private parent")?
            .parse::<u32>()?;
        let name = args[2].to_str().context("Invalid private board name")?;
        let suffix = name
            .strip_prefix("screenshots.owned-image-text-fixture.")
            .context("Only a private named board is admitted")?;
        let _ = uuid::Uuid::parse_str(suffix)?;
        let _lifeline = transport::Lifeline::new(parent)?;
        let _budget = budget::Budget::enter()?;
        return native_round_trip(name);
    }
    let name = format!(
        "screenshots.owned-image-text-fixture.{}",
        uuid::Uuid::new_v4()
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut command = Command::new(std::env::current_exe()?);
    command
        .arg(CHILD)
        .arg(std::process::id().to_string())
        .arg(name)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0);
    for key in ["HOME", "PATH", "LANG", "LC_ALL", "LC_CTYPE", "TMPDIR"] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    let mut child = command.spawn()?;
    let group = match i32::try_from(child.id()) {
        Ok(group) => group,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error.into());
        }
    };
    let mut native = Native {
        child,
        group,
        status: None,
    };
    loop {
        if unix::exited_without_reaping(group, false)? {
            ensure!(
                native.reap().success(),
                "Owned native pasteboard child failed"
            );
            break;
        }
        if Instant::now() >= deadline {
            let _ = native.reap();
            anyhow::bail!(
                "Owned native pasteboard child exceeded its five-second deadline; actual cleanup completed"
            );
        }
        std::thread::park_timeout(Duration::from_millis(5));
    }
    // The controller never invokes a Pasteboard/CoreFoundation publisher, reader or cleanup API.
    println!(
        "{{\"owned_named_pasteboard\":true,\"image_alpha\":true,\"text_replacement\":true,\"child_reaped\":true}}"
    );
    Ok(())
}

#[cfg(target_os = "macos")]
fn native_round_trip(name: &str) -> anyhow::Result<()> {
    use anyhow::ensure;
    use std::{
        ffi::{CString, c_void},
        io::Cursor,
    };
    type Cf = *const c_void;
    type Board = *mut c_void;
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFStringCreateWithCString(a: Cf, bytes: *const i8, encoding: u32) -> Cf;
        fn CFRelease(value: Cf);
        fn CFDataGetLength(data: Cf) -> isize;
        fn CFDataGetBytePtr(data: Cf) -> *const u8;
    }
    #[link(name = "ApplicationServices", kind = "framework")]
    unsafe extern "C" {
        fn PasteboardCreate(name: Cf, board: *mut Board) -> i32;
        fn PasteboardSynchronize(board: Board) -> u32;
        fn PasteboardCopyItemFlavorData(
            board: Board,
            item: usize,
            flavor: Cf,
            data: *mut Cf,
        ) -> i32;
    }
    struct Owned(Cf);
    impl Drop for Owned {
        fn drop(&mut self) {
            unsafe {
                CFRelease(self.0);
            }
        }
    }
    fn string(value: &str) -> anyhow::Result<Owned> {
        let value = CString::new(value)?;
        let value =
            unsafe { CFStringCreateWithCString(std::ptr::null(), value.as_ptr(), 0x08000100) };
        ensure!(!value.is_null(), "Private pasteboard string failed");
        Ok(Owned(value))
    }
    fn read(name: &str, flavor: &str, limit: usize) -> anyhow::Result<Vec<u8>> {
        ensure!(
            name.starts_with("screenshots.owned-image-text-fixture."),
            "Only a private named pasteboard is admitted"
        );
        let name = string(name)?;
        let flavor = string(flavor)?;
        let mut board = std::ptr::null_mut();
        ensure!(
            unsafe { PasteboardCreate(name.0, &mut board) } == 0 && !board.is_null(),
            "Private board unavailable"
        );
        let _board = Owned(board.cast());
        let _ = unsafe { PasteboardSynchronize(board) };
        let mut data = std::ptr::null();
        ensure!(
            unsafe { PasteboardCopyItemFlavorData(board, 1, flavor.0, &mut data) } == 0
                && !data.is_null(),
            "Private native paste failed"
        );
        let data = Owned(data);
        let length = unsafe { CFDataGetLength(data.0) };
        ensure!(
            length >= 0 && length as usize <= limit,
            "Private native paste exceeded its bound"
        );
        if length == 0 {
            return Ok(Vec::new());
        }
        let bytes = unsafe { CFDataGetBytePtr(data.0) };
        ensure!(!bytes.is_null(), "Private native paste data missing");
        Ok(unsafe { std::slice::from_raw_parts(bytes, length as usize) }.to_vec())
    }
    let pixels = image::RgbaImage::from_fn(3, 2, |x, y| image::Rgba([x as u8, y as u8, 47, 83]));
    let mut png = Cursor::new(Vec::new());
    pixels.write_to(&mut png, image::ImageFormat::Png)?;
    publisher::publish_bytes(png.get_ref(), "public.png", &name)?;
    drop(png);
    let pasted = read(&name, "public.png", 65536)?;
    ensure!(
        image::load_from_memory(&pasted)?.into_rgba8() == pixels,
        "Private PNG pixels differ"
    );
    let text = "owned UTF-8 ä 🐾";
    publisher::publish_bytes(text.as_bytes(), "public.utf8-plain-text", &name)?;
    ensure!(
        read(&name, "public.utf8-plain-text", 65536)? == text.as_bytes(),
        "Private text differs"
    );
    Ok(())
}
#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("Native macOS named-pasteboard fixture only; no clipboard fallback.");
}
