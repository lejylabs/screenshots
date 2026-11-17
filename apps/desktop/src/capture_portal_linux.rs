//! Operation-owned restricted portal source; still, finite video, or manual scroll assembly.
use app_core::{
    CaptureError, CapturedImage, PixelFormat, PortalCaptureCursor, PortalCaptureSource,
};
use ashpd::desktop::{
    PersistMode,
    screencast::{CursorMode, Screencast, SelectSourcesOptions, SourceType},
};
use futures_lite::{Stream, StreamExt, future};
use pipewire::{self as pw, properties::properties, spa};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    future::Future,
    io,
    io::Write,
    os::fd::{AsRawFd, OwnedFd},
    os::unix::net::UnixStream,
    path::PathBuf,
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};
#[path = "capture_portal_formats.rs"]
mod formats;
#[path = "capture_portal_pixels.rs"]
mod pixels;
#[repr(transparent)]
struct Transform(u32);
impl spa::buffer::meta::Metadata for Transform {
    const META_TYPE: u32 = spa::sys::SPA_META_VideoTransform;
}
fn failed() -> CaptureError {
    CaptureError::Backend("Portal screen capture failed".into())
}
fn portal_error(error: ashpd::Error) -> CaptureError {
    match error {
        ashpd::Error::Response(ashpd::desktop::ResponseError::Cancelled)
        | ashpd::Error::Portal(
            ashpd::PortalError::NotAllowed(_)
            | ashpd::PortalError::Cancelled(_)
            | ashpd::PortalError::WindowDestroyed(_),
        ) => CaptureError::PermissionDenied,
        _ => failed(),
    }
}
async fn bounded<T>(
    until: Instant,
    work: impl Future<Output = Result<T, CaptureError>>,
) -> Result<T, CaptureError> {
    future::or(work, async {
        async_io::Timer::at(until).await;
        Err(CaptureError::NativeTimeout)
    })
    .await
}
fn source_type(source: PortalCaptureSource) -> SourceType {
    match source {
        PortalCaptureSource::Display => SourceType::MONITOR,
        PortalCaptureSource::Window => SourceType::WINDOW,
    }
}
fn timestamp_policy() -> CaptureError {
    CaptureError::Unsupported("Portal video requires non-future CLOCK_MONOTONIC source Header PTS")
}
fn monotonic_ns() -> Result<i64, CaptureError> {
    let mut time: libc::timespec = unsafe { std::mem::zeroed() };
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut time) } != 0 {
        return Err(timestamp_policy());
    }
    let seconds = i64::try_from(time.tv_sec).map_err(|_| timestamp_policy())?;
    let nanos = i64::try_from(time.tv_nsec).map_err(|_| timestamp_policy())?;
    seconds
        .checked_mul(1_000_000_000)
        .and_then(|value| value.checked_add(nanos))
        .filter(|value| *value > 0)
        .ok_or_else(timestamp_policy)
}
/// Made only after the authenticated VIDEO_START, never from a frame callback.
#[derive(Clone, Copy)]
pub(super) struct VideoStart {
    cutoff_ns: i64,
}
impl VideoStart {
    pub(super) fn after_encoder_start() -> Result<Self, CaptureError> {
        Ok(Self {
            cutoff_ns: monotonic_ns()?,
        })
    }
    #[cfg(test)]
    pub(super) fn cutoff_ns(self) -> i64 {
        self.cutoff_ns
    }
}
#[derive(Default)]
struct VideoClock {
    previous: Option<i64>,
    start: Option<VideoStart>,
    fresh: bool,
}
impl VideoClock {
    fn admit(&mut self, pts: Option<i64>, now_ns: i64) -> Result<bool, CaptureError> {
        let pts = pts
            .filter(|pts| *pts > 0 && *pts <= now_ns)
            .ok_or_else(timestamp_policy)?;
        if self.start.is_some_and(|start| pts <= start.cutoff_ns) {
            if self.fresh {
                return Err(timestamp_policy());
            }
            return Ok(false);
        }
        if self.previous.is_some_and(|previous| pts <= previous) {
            return Err(timestamp_policy());
        }
        self.previous = Some(pts);
        self.fresh = self.start.is_some();
        Ok(true)
    }
}
enum Output {
    Frame(image::RgbaImage),
    Video(app_core::PortalVideoSummary),
    Scroll(image::RgbaImage, app_core::PortalScrollSummary),
}
enum Mode<'a> {
    Still,
    Video {
        request: app_core::PortalVideoRequest,
        sink: UnixStream,
        ready: &'a mut dyn FnMut() -> Result<VideoStart, CaptureError>,
    },
    Scroll(app_core::PortalScrollRequest),
}
impl Mode<'_> {
    fn source(&self) -> Option<app_core::PortalStreamSource> {
        match self {
            Self::Still => None,
            Self::Video { request, .. } => Some(request.stream),
            Self::Scroll(request) => Some(request.stream),
        }
    }
}
fn captured(
    source: PortalCaptureSource,
    pixels: image::RgbaImage,
    started: Instant,
) -> CapturedImage {
    CapturedImage {
        desktop_layout: None,
        display_id: source.identifier().into(),
        window_id: None,
        width: pixels.width(),
        height: pixels.height(),
        png_path: PathBuf::new(),
        pixels: Arc::new(pixels),
        elapsed_ms: started.elapsed().as_millis(),
        source_format: PixelFormat::Rgba8UnormSrgb,
        display_geometry: None,
    }
}
pub(super) fn scroll(
    request: app_core::PortalScrollRequest,
    until: Instant,
) -> Result<app_core::NativeCaptureReply, CaptureError> {
    request.validate()?;
    let started = Instant::now();
    let Output::Scroll(pixels, summary) = run(
        request.stream.source,
        request.stream.cursor,
        until,
        Mode::Scroll(request),
    )?
    else {
        return Err(failed());
    };
    Ok(app_core::NativeCaptureReply::PortalScroll {
        image: captured(request.stream.source, pixels, started),
        summary,
    })
}
pub(super) fn video(
    request: app_core::PortalVideoRequest,
    until: Instant,
    sink: UnixStream,
    ready: &mut dyn FnMut() -> Result<VideoStart, CaptureError>,
) -> Result<app_core::PortalVideoSummary, CaptureError> {
    request.validate()?;
    let Output::Video(summary) = run(
        request.stream.source,
        request.stream.cursor,
        until,
        Mode::Video {
            request,
            sink,
            ready,
        },
    )?
    else {
        return Err(failed());
    };
    Ok(summary)
}
pub(super) fn capture(
    source: PortalCaptureSource,
    cursor: PortalCaptureCursor,
    until: Instant,
) -> Result<CapturedImage, CaptureError> {
    let started = Instant::now();
    let Output::Frame(pixels) = run(source, cursor, until, Mode::Still)? else {
        return Err(failed());
    };
    Ok(captured(source, pixels, started))
}
fn run(
    source: PortalCaptureSource,
    cursor: PortalCaptureCursor,
    until: Instant,
    mode: Mode<'_>,
) -> Result<Output, CaptureError> {
    let consent_until = until.min(Instant::now() + Duration::from_secs(120));
    ashpd::zbus::block_on(async {
        let proxy = bounded(consent_until, async {
            Screencast::new().await.map_err(portal_error)
        })
        .await?;
        if proxy.version() < 3 {
            return Err(CaptureError::Unsupported(
                "Portal ScreenCast version 3 or newer is required",
            ));
        }
        let sources = bounded(consent_until, async {
            proxy.available_source_types().await.map_err(portal_error)
        })
        .await?;
        let cursors = bounded(consent_until, async {
            proxy.available_cursor_modes().await.map_err(portal_error)
        })
        .await?;
        let cursor = match cursor {
            PortalCaptureCursor::Exclude => CursorMode::HIDDEN,
            PortalCaptureCursor::Include => CursorMode::EMBEDDED,
        };
        if !sources.contains(source_type(source)) || !cursors.contains(cursor) {
            return Err(CaptureError::Unsupported(
                "Portal does not offer the requested source/cursor policy",
            ));
        }
        let session = bounded(consent_until, async {
            proxy
                .create_session(Default::default())
                .await
                .map_err(portal_error)
        })
        .await?;
        let work = async {
            let closed = session.receive_closed().await.map_err(portal_error)?;
            let mut closed = std::pin::pin!(closed);
            let capture = async {
                proxy
                    .select_sources(
                        &session,
                        SelectSourcesOptions::default()
                            .set_sources(source_type(source))
                            .set_multiple(false)
                            .set_cursor_mode(cursor)
                            .set_persist_mode(PersistMode::DoNot),
                    )
                    .await
                    .map_err(portal_error)?
                    .response()
                    .map_err(portal_error)?;
                let streams = proxy
                    .start(&session, None, Default::default())
                    .await
                    .map_err(portal_error)?
                    .response()
                    .map_err(portal_error)?;
                if streams.streams().len() != 1 {
                    return Err(failed());
                }
                let stream = &streams.streams()[0];
                if stream.pipe_wire_node_id() == u32::MAX
                    || stream
                        .source_type()
                        .is_some_and(|kind| kind != source_type(source))
                    || (proxy.version() >= 6 && stream.pipewire_serial().is_none())
                    || stream.pipewire_serial() == Some(0)
                {
                    return Err(failed());
                }
                let remote = proxy
                    .open_pipe_wire_remote(&session, Default::default())
                    .await
                    .map_err(portal_error)?;
                validate_remote(&remote)?;
                Ok((remote, stream.pipe_wire_node_id(), stream.pipewire_serial()))
            };
            let (remote, node, serial) = bounded(
                consent_until,
                future::or(capture, async {
                    closed.next().await;
                    Err(CaptureError::PermissionDenied)
                }),
            )
            .await?;
            receive_frame(remote, node, serial, &mut closed, until, mode).await
        };
        let result = bounded(until, work).await;
        // Native/session resources must be gone before publishing a frame to the parent.
        let close = bounded(until.min(Instant::now() + Duration::from_secs(3)), async {
            session.close().await.map_err(portal_error)
        })
        .await;
        match (result, close) {
            (Ok(frame), Ok(())) => Ok(frame),
            (Err(error), _) => Err(error),
            (Ok(_), Err(_)) => Err(failed()),
        }
    })
}

fn validate_remote(remote: &OwnedFd) -> Result<(), CaptureError> {
    let fd = remote.as_raw_fd();
    let mut info: libc::stat = unsafe { std::mem::zeroed() };
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    let mut kind = 0i32;
    let mut address: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
    let mut address_length = std::mem::size_of_val(&address) as libc::socklen_t;
    let mut length = std::mem::size_of_val(&kind) as libc::socklen_t;
    if fd < 3
        || flags < 0
        || unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) } != 0
        || unsafe { libc::fstat(fd, &mut info) } != 0
        || info.st_mode & libc::S_IFMT != libc::S_IFSOCK
        || unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_TYPE,
                (&mut kind as *mut i32).cast(),
                &mut length,
            )
        } != 0
        || kind != libc::SOCK_STREAM
        || unsafe {
            libc::getpeername(
                fd,
                (&mut address as *mut libc::sockaddr_storage).cast(),
                &mut address_length,
            )
        } != 0
        || address.ss_family as i32 != libc::AF_UNIX
    {
        return Err(failed());
    }
    Ok(())
}
#[derive(Default)]
struct State {
    error: Option<CaptureError>,
    serial: Option<u64>,
    seen: usize,
    globals: usize,
    format: Option<pixels::Format>,
    buffers: HashMap<usize, usize>,
    mapped: usize,
    frame: Option<Arc<image::RgbaImage>>,
    source: Option<app_core::PortalStreamSource>,
    decoded: u32,
    received: u32,
    timestamp: Option<i64>,
    video_clock: Option<VideoClock>,
    streaming: bool,
    finished: bool,
}
impl State {
    fn fail(&mut self, error: CaptureError) {
        if self.error.is_none() {
            self.error = Some(error);
        }
    }
    fn check(&mut self) -> Result<(), CaptureError> {
        self.error.take().map_or(Ok(()), Err)
    }
}
async fn drive(
    main: &pw::main_loop::MainLoopRc,
    state: &Rc<RefCell<State>>,
    complete: impl Fn() -> bool,
    closed: &mut (impl Stream + Unpin),
    until: Instant,
) -> Result<(), CaptureError> {
    let fd = main
        .loop_()
        .fd()
        .try_clone_to_owned()
        .map_err(|_| failed())?;
    let ready = async_io::Async::new(fd).map_err(|_| failed())?;
    bounded(
        until,
        future::or(
            async {
                ready
                    .read_with(|_| {
                        let result = main.loop_().iterate(pw::loop_::Timeout::None);
                        if result < 0 {
                            return Err(io::Error::other("Native frame loop failed"));
                        }
                        if state.borrow().error.is_some() || complete() {
                            Ok(())
                        } else {
                            Err(io::ErrorKind::WouldBlock.into())
                        }
                    })
                    .await
                    .map_err(|_| failed())?;
                state.borrow_mut().check()
            },
            async {
                closed.next().await;
                Err(CaptureError::PermissionDenied)
            },
        ),
    )
    .await
}
async fn receive_frame(
    remote: OwnedFd,
    target: u32,
    expected_serial: Option<u64>,
    closed: &mut (impl Stream + Unpin),
    overall: Instant,
    mut mode: Mode<'_>,
) -> Result<Output, CaptureError> {
    pw::init();
    let until = overall.min(Instant::now() + Duration::from_secs(20));
    let main = pw::main_loop::MainLoopRc::new(None).map_err(|_| failed())?;
    let context = pw::context::ContextRc::new(&main, None).map_err(|_| failed())?;
    let core = context.connect_fd_rc(remote, None).map_err(|_| failed())?;
    let state = Rc::new(RefCell::new(State {
        source: mode.source(),
        video_clock: matches!(&mode, Mode::Video { .. }).then(VideoClock::default),
        ..State::default()
    }));

    let done = Rc::new(Cell::new(false));
    let pending = Rc::new(Cell::new(None));
    let events = state.clone();
    let complete = done.clone();
    let sequence = pending.clone();
    let _core = core
        .add_listener_local()
        .error(move |_, _, _, _| events.borrow_mut().fail(failed()))
        .done(move |id, seq| {
            if id == pw::core::PW_ID_CORE && sequence.get() == Some(seq) {
                complete.set(true);
            }
        })
        .register();
    let registry = core.get_registry().map_err(|_| failed())?;
    let events = state.clone();
    let removed = state.clone();
    let _registry = registry
        .add_listener_local()
        .global(move |object| {
            let mut state = events.borrow_mut();
            state.globals += 1;
            if state.globals > 8192 {
                state.fail(failed());
                return;
            }
            if object.id != target {
                return;
            }
            state.seen += 1;
            if object.type_ != pw::types::ObjectType::Node || state.seen != 1 {
                state.fail(failed());
                return;
            }
            let serial = object
                .props
                .as_ref()
                .and_then(|props| props.get("object.serial"))
                .filter(|value| value.len() <= 20)
                .and_then(|value| value.parse::<u64>().ok())
                .filter(|value| *value != 0);
            if serial.is_none() || expected_serial.is_some_and(|expected| Some(expected) != serial)
            {
                state.fail(failed());
            } else {
                state.serial = serial;
            }
        })
        .global_remove(move |id| {
            if id == target {
                removed.borrow_mut().fail(failed());
            }
        })
        .register();
    pending.set(Some(core.sync(0).map_err(|_| failed())?));
    drive(&main, &state, || done.get(), closed, until).await?;
    let serial = state.borrow().serial.ok_or_else(failed)?;
    let stream = pw::stream::StreamBox::new(
        &core,
        "screenshots-one-frame",
        properties! {
            *pw::keys::MEDIA_TYPE => "Video", *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Screen", "target.object" => serial.to_string(),
        "node.name" => "screenshots-one-frame",
            "node.dont-reconnect" => "true", "node.latency" => "1/30",
        },
    )
    .map_err(|_| failed())?;
    let data = state.clone();
    let _stream = stream
        .add_local_listener_with_user_data(data)
        .state_changed(|_, state, _, new| {
            let mut state = state.borrow_mut();
            if matches!(new, pw::stream::StreamState::Streaming) {
                state.streaming = true;
            }
            if matches!(new, pw::stream::StreamState::Error(_))
                || (state.source.is_some()
                    && state.streaming
                    && !state.finished
                    && matches!(
                        new,
                        pw::stream::StreamState::Paused | pw::stream::StreamState::Unconnected
                    ))
            {
                state.fail(failed());
            }
        })
        .param_changed(|stream, state, id, param| {
            if id != spa::param::ParamType::Format.as_raw() {
                return;
            }
            let result = (|| {
                let param = param.ok_or_else(pixels::unsupported)?;
                let media = spa::param::format_utils::parse_format(param)
                    .map_err(|_| pixels::unsupported())?;
                if media
                    != (
                        spa::param::format::MediaType::Video,
                        spa::param::format::MediaSubtype::Raw,
                    )
                {
                    return Err(pixels::unsupported());
                }
                let mut info = spa::param::video::VideoInfoRaw::new();
                info.parse(param).map_err(|_| pixels::unsupported())?;
                let format = pixels::Format::parse(info)?;
                if state.borrow().source.is_some_and(|source| {
                    (source.width, source.height) != (format.width, format.height)
                }) {
                    return Err(pixels::unsupported());
                }
                if state
                    .borrow()
                    .format
                    .is_some_and(|previous| previous != format)
                {
                    return Err(pixels::unsupported());
                }
                let buffers = buffer_parameters(format)?;
                let header = if state.borrow().video_clock.is_some() {
                    Some(header_parameter()?)
                } else {
                    None
                };
                let mut params = vec![spa::pod::Pod::from_bytes(&buffers).ok_or_else(failed)?];
                if let Some(header) = &header {
                    params.push(spa::pod::Pod::from_bytes(header).ok_or_else(failed)?);
                }
                stream.update_params(&mut params).map_err(|_| failed())?;
                state.borrow_mut().format = Some(format);
                Ok(())
            })();
            if let Err(error) = result {
                state.borrow_mut().fail(error);
            }
        })
        .add_buffer(|_, state, buffer| {
            let result = unsafe { inspect_buffer(buffer) };
            let mut state = state.borrow_mut();
            match result {
                Ok(size)
                    if (state.source.is_none()
                        || state
                            .format
                            .is_some_and(|format| size <= format.finite_buffer_limit()))
                        && state.buffers.len() < 2
                        && !state.buffers.contains_key(&(buffer as usize))
                        && state
                            .mapped
                            .checked_add(size)
                            .is_some_and(|total| total <= pixels::MAX_MAPPED_BYTES * 2) =>
                {
                    state.buffers.insert(buffer as usize, size);
                    state.mapped += size;
                }
                _ => state.fail(pixels::unsupported()),
            }
        })
        .remove_buffer(|_, state, buffer| {
            let mut state = state.borrow_mut();
            if let Some(size) = state.buffers.remove(&(buffer as usize)) {
                state.mapped -= size;
            }
        })
        .process(|stream, state| {
            if state.borrow().finished
                || (state.borrow().source.is_none() && state.borrow().frame.is_some())
                || state.borrow().error.is_some()
            {
                return;
            }
            let Some(mut buffer) = stream.dequeue_buffer() else {
                return;
            };
            let result = (|| {
                let format = state.borrow().format.ok_or_else(pixels::unsupported)?;
                if state.borrow().source.is_some() {
                    let mut state = state.borrow_mut();
                    if state.received >= 16384 {
                        return Err(CaptureError::MemoryLimit);
                    }
                    state.received += 1;
                }
                if buffer
                    .find_meta::<Transform>()
                    .is_some_and(|transform| transform.0 != 0)
                {
                    return Err(pixels::unsupported());
                }
                let header = buffer.find_meta::<spa::buffer::meta::MetaHeader>();
                let current_ns = if state.borrow().video_clock.is_some() {
                    monotonic_ns()?
                } else {
                    0
                };
                if state.borrow().video_clock.is_some() {
                    let header = header.ok_or_else(timestamp_policy)?;
                    if header.offset() != 0
                        || header.dts_offset() != 0
                        || (state
                            .borrow()
                            .video_clock
                            .as_ref()
                            .is_some_and(|clock| clock.fresh)
                            && header
                                .flags()
                                .contains(spa::buffer::meta::MetaHeaderFlags::DISCONT))
                    {
                        return Err(timestamp_policy());
                    }
                }
                if let Some(clock) = state.borrow_mut().video_clock.as_mut() {
                    if !clock.admit(header.map(|header| header.pts()), current_ns)? {
                        return Ok(None);
                    }
                }
                if let Some(header) = header {
                    if state.borrow().source.is_some() && state.borrow().video_clock.is_none() {
                        let mut state = state.borrow_mut();
                        if header.pts() < 0
                            || state
                                .timestamp
                                .is_some_and(|previous| header.pts() <= previous)
                            || (state.decoded != 0
                                && header
                                    .flags()
                                    .contains(spa::buffer::meta::MetaHeaderFlags::DISCONT))
                        {
                            return Err(pixels::unsupported());
                        }
                        state.timestamp = Some(header.pts());
                    }
                    if header.flags().bits()
                        & !(spa::buffer::meta::MetaHeaderFlags::DISCONT
                            | spa::buffer::meta::MetaHeaderFlags::MARKER)
                            .bits()
                        != 0
                    {
                        return Err(pixels::unsupported());
                    }
                }
                if let Some(crop) = buffer.find_meta::<spa::buffer::meta::MetaVideoCrop>() {
                    let region = crop.meta_region();
                    let position = region.position();
                    let size = region.size();
                    if region.is_valid()
                        && (position.x != 0
                            || position.y != 0
                            || size.width != format.width
                            || size.height != format.height)
                    {
                        return Err(pixels::unsupported());
                    }
                }
                let datas = buffer.datas_mut();
                if datas.len() != 1 {
                    return Err(pixels::unsupported());
                }
                let data = &datas[0];
                let raw = data.as_raw();
                if raw.chunk.is_null()
                    || raw.data.is_null()
                    || raw.maxsize as usize > pixels::MAX_MAPPED_BYTES
                    || !matches!(
                        data.type_(),
                        spa::buffer::DataType::MemPtr | spa::buffer::DataType::MemFd
                    )
                    || !data.flags().contains(spa::buffer::DataFlags::READABLE)
                {
                    return Err(pixels::unsupported());
                }
                let chunk = data.chunk();
                if !chunk.flags().is_empty() {
                    return Err(pixels::unsupported());
                }
                if chunk.size() == 0 {
                    return Ok(None);
                }
                let extent = format.extent(
                    raw.maxsize as usize,
                    chunk.offset(),
                    chunk.size(),
                    chunk.stride(),
                )?;
                // Buffer ownership, CPU mapping, exact extent and format are admitted first.
                let bytes = unsafe {
                    std::slice::from_raw_parts(
                        (raw.data as *const u8).add(extent.start),
                        extent.len(),
                    )
                };
                Ok(Some(format.decode(bytes, chunk.stride() as usize)?))
            })();
            drop(buffer); // Return the sole consumed native buffer before deactivation.
            match result {
                Ok(Some(frame)) => {
                    {
                        let mut state = state.borrow_mut();
                        state.frame = Some(Arc::new(frame));
                        state.decoded += 1;
                    }
                    if state.borrow().source.is_none() && stream.set_active(false).is_err() {
                        state.borrow_mut().fail(failed());
                    }
                }
                Ok(None) => {}
                Err(error) => state.borrow_mut().fail(error),
            }
        })
        .register()
        .map_err(|_| failed())?;
    let formats = formats::offers(mode.source())?;
    let mut parameters = formats
        .iter()
        .map(|bytes| spa::pod::Pod::from_bytes(bytes).ok_or_else(failed))
        .collect::<Result<Vec<_>, _>>()?;
    stream
        .connect(
            spa::utils::Direction::Input,
            None,
            pw::stream::StreamFlags::AUTOCONNECT
                | pw::stream::StreamFlags::MAP_BUFFERS
                | pw::stream::StreamFlags::DONT_RECONNECT,
            &mut parameters,
        )
        .map_err(|_| failed())?;
    drive(
        &main,
        &state,
        || state.borrow().frame.is_some(),
        closed,
        until,
    )
    .await?;
    if let Mode::Video { ready, .. } = &mut mode {
        let start = ready()?;
        {
            let mut state = state.borrow_mut();
            state.frame = None;
            state.video_clock.as_mut().ok_or_else(failed)?.start = Some(start);
        }
        // Flush the native consumer queue; PTS also excludes older upstream/converter frames.
        stream.flush(false).map_err(|_| failed())?;
        drive(
            &main,
            &state,
            || state.borrow().frame.is_some(),
            closed,
            overall.min(Instant::now() + Duration::from_secs(5)),
        )
        .await?;
    }
    let output = match mode {
        Mode::Still => Output::Frame(
            Arc::try_unwrap(state.borrow_mut().frame.take().ok_or_else(failed)?)
                .map_err(|_| failed())?,
        ),
        Mode::Video { request, sink, .. } => {
            let sink = async_io::Async::new(sink).map_err(|_| failed())?;
            let started = Instant::now();
            let active = overall.min(started + Duration::from_secs(u64::from(request.seconds) + 5));
            let expected = request.frames()?;
            let mut previous = 0;
            let mut repeated = 0;
            let mut last_sample = Duration::ZERO;
            for index in 0..expected {
                let tick = started
                    + Duration::from_nanos(
                        u64::from(index) * 1_000_000_000 / u64::from(request.fps),
                    );
                wait_sample(&main, &state, closed, tick, active).await?;
                let sample = started.elapsed();
                let nominal = tick.saturating_duration_since(started);
                if sample.saturating_sub(nominal)
                    > Duration::from_nanos(1_000_000_000 / u64::from(request.fps) + 50_000_000)
                {
                    return Err(CaptureError::NativeTimeout);
                }
                let (frame, generation) = {
                    let state = state.borrow();
                    (state.frame.clone().ok_or_else(failed)?, state.decoded)
                };
                if previous == generation {
                    repeated += 1;
                }
                previous = generation;
                // Reuse the one retained frame; the encoder consumes packed BGRA.
                let mut bytes = frame.as_raw().clone();
                for pixel in bytes.chunks_exact_mut(4) {
                    for channel in 0..3 {
                        pixel[channel] =
                            ((u16::from(pixel[channel]) * u16::from(pixel[3]) + 127) / 255) as u8;
                    }
                    pixel.swap(0, 2);
                    pixel[3] = 255;
                }
                bounded(
                    active,
                    future::or(
                        async {
                            let mut left = bytes.as_slice();
                            while !left.is_empty() {
                                let count = sink
                                    .write_with(|socket| {
                                        (&*socket).write(&left[..left.len().min(65536)])
                                    })
                                    .await
                                    .map_err(|_| failed())?;
                                if count == 0 {
                                    return Err(failed());
                                }
                                left = &left[count..];
                            }
                            Ok(())
                        },
                        async {
                            closed.next().await;
                            Err(CaptureError::PermissionDenied)
                        },
                    ),
                )
                .await?;
                last_sample = sample;
            }
            drop(sink);
            let summary = app_core::PortalVideoSummary {
                frames: expected,
                decoded_frames: state.borrow().decoded,
                repeated_frames: repeated,
                sample_span_ns: last_sample.as_nanos().try_into().map_err(|_| failed())?,
            };
            summary.validate(request)?;
            Output::Video(summary)
        }
        Mode::Scroll(request) => {
            let started = Instant::now();
            let active = overall.min(started + Duration::from_secs(65));
            let first = (*state.borrow().frame.clone().ok_or_else(failed)?).clone();
            let mut assembly = app_core::ScrollAssembler::new(
                first,
                app_core::ScrollMatchOptions {
                    minimum_overlap: request.minimum_overlap,
                    tolerance: 0,
                    trim_top: 0,
                    trim_bottom: 0,
                },
            )
            .map_err(|_| failed())?;
            for index in 1..request.frames {
                let tick = started
                    + Duration::from_millis(u64::from(index) * u64::from(request.interval_ms));
                wait_sample(&main, &state, closed, tick, active).await?;
                let frame = (*state.borrow().frame.clone().ok_or_else(failed)?).clone();
                assembly
                    .push(frame, || Instant::now() >= active)
                    .map_err(|_| failed())?;
            }
            let summary = app_core::PortalScrollSummary {
                captured_frames: request.frames,
                vertical_advances: assembly.advances().to_vec(),
            };
            let pixels = assembly.finish().map_err(|_| failed())?;
            summary.validate(request, pixels.width(), pixels.height())?;
            Output::Scroll(pixels, summary)
        }
    };
    state.borrow_mut().finished = true;
    stream.set_active(false).map_err(|_| failed())?;
    done.set(false);
    pending.set(Some(core.sync(0).map_err(|_| failed())?));
    drive(
        &main,
        &state,
        || done.get(),
        closed,
        overall.min(Instant::now() + Duration::from_secs(3)),
    )
    .await?;
    Ok(output)
}
// Active finite sampling only: PipeWire readiness, portal closure, or the next sample deadline.
async fn wait_sample(
    main: &pw::main_loop::MainLoopRc,
    state: &Rc<RefCell<State>>,
    closed: &mut (impl Stream + Unpin),
    tick: Instant,
    until: Instant,
) -> Result<(), CaptureError> {
    bounded(
        until,
        future::or(drive(main, state, || false, closed, until), async {
            async_io::Timer::at(tick).await;
            Ok(())
        }),
    )
    .await?;
    if main.loop_().iterate(pw::loop_::Timeout::None) < 0 {
        return Err(failed());
    }
    state.borrow_mut().check()
}

unsafe fn inspect_buffer(buffer: *mut pw::sys::pw_buffer) -> Result<usize, CaptureError> {
    if buffer.is_null() {
        return Err(pixels::unsupported());
    }
    let buffer = unsafe { (*buffer).buffer };
    if buffer.is_null() {
        return Err(pixels::unsupported());
    }
    let raw = unsafe { &*buffer };
    if raw.n_datas != 1
        || raw.datas.is_null()
        || raw.n_metas > 16
        || (raw.n_metas != 0 && raw.metas.is_null())
    {
        return Err(pixels::unsupported());
    }
    if raw.n_metas != 0 {
        // Metadata descriptors are owned/stable for this registered native buffer.
        for meta in unsafe { std::slice::from_raw_parts(raw.metas, raw.n_metas as usize) } {
            if meta.type_ == spa::sys::SPA_META_VideoTransform {
                if meta.data.is_null()
                    || meta.size < std::mem::size_of::<u32>() as u32
                    || unsafe { std::ptr::read_unaligned(meta.data.cast::<u32>()) } != 0
                {
                    return Err(pixels::unsupported());
                }
            }
        }
    }
    let data = unsafe { &*raw.datas };
    if data.maxsize == 0
        || data.maxsize as usize > pixels::MAX_MAPPED_BYTES
        || !matches!(
            data.type_,
            spa::sys::SPA_DATA_MemPtr | spa::sys::SPA_DATA_MemFd
        )
        || data.chunk.is_null()
        || data.data.is_null()
    {
        return Err(pixels::unsupported());
    }
    Ok(data.maxsize as usize)
}
fn serialize(object: spa::pod::Object) -> Result<Vec<u8>, CaptureError> {
    spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &spa::pod::Value::Object(object),
    )
    .map(|value| value.0.into_inner())
    .map_err(|_| failed())
}
fn header_parameter() -> Result<Vec<u8>, CaptureError> {
    serialize(spa::pod::Object {
        type_: spa::utils::SpaTypes::ObjectParamMeta.as_raw(),
        id: spa::param::ParamType::Meta.as_raw(),
        properties: vec![
            spa::pod::Property::new(
                spa::sys::SPA_PARAM_META_type,
                spa::pod::Value::Id(spa::utils::Id(spa::sys::SPA_META_Header)),
            ),
            spa::pod::Property::new(
                spa::sys::SPA_PARAM_META_size,
                spa::pod::Value::Int(std::mem::size_of::<spa::sys::spa_meta_header>() as i32),
            ),
        ],
    })
}
fn buffer_parameters(format: pixels::Format) -> Result<Vec<u8>, CaptureError> {
    let size = format
        .width
        .checked_mul(format.height)
        .and_then(|value| value.checked_mul(format.packing.bytes_per_pixel() as u32))
        .ok_or(CaptureError::MemoryLimit)?;
    serialize(spa::pod::Object {
        type_: spa::utils::SpaTypes::ObjectParamBuffers.as_raw(),
        id: spa::param::ParamType::Buffers.as_raw(),
        properties: vec![
            spa::pod::Property::new(
                spa::sys::SPA_PARAM_BUFFERS_buffers,
                spa::pod::Value::Choice(spa::pod::ChoiceValue::Int(spa::utils::Choice(
                    spa::utils::ChoiceFlags::empty(),
                    spa::utils::ChoiceEnum::Range {
                        default: 2,
                        min: 1,
                        max: 2,
                    },
                ))),
            ),
            spa::pod::Property::new(spa::sys::SPA_PARAM_BUFFERS_blocks, spa::pod::Value::Int(1)),
            spa::pod::Property::new(
                spa::sys::SPA_PARAM_BUFFERS_size,
                spa::pod::Value::Int(size as i32),
            ),
            spa::pod::Property::new(
                spa::sys::SPA_PARAM_BUFFERS_stride,
                spa::pod::Value::Int(
                    (format.width * format.packing.bytes_per_pixel() as u32) as i32,
                ),
            ),
            spa::pod::Property::new(
                spa::sys::SPA_PARAM_BUFFERS_dataType,
                spa::pod::Value::Int(
                    (1 << spa::sys::SPA_DATA_MemPtr | 1 << spa::sys::SPA_DATA_MemFd) as i32,
                ),
            ),
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_generic_portal_failure_is_not_reported_as_a_permission_denial() {
        assert!(matches!(
            portal_error(ashpd::Error::Response(ashpd::desktop::ResponseError::Other)),
            CaptureError::Backend(_)
        ));
        assert!(matches!(
            portal_error(ashpd::Error::Response(
                ashpd::desktop::ResponseError::Cancelled
            )),
            CaptureError::PermissionDenied
        ));
    }
    #[test]
    fn remote_must_be_a_private_stream_socket_and_is_immediately_noninheritable() {
        let (stream, _peer) = std::os::unix::net::UnixStream::pair().unwrap();
        let remote: OwnedFd = stream.into();
        let flags = unsafe { libc::fcntl(remote.as_raw_fd(), libc::F_GETFD) };
        assert_eq!(
            unsafe { libc::fcntl(remote.as_raw_fd(), libc::F_SETFD, flags & !libc::FD_CLOEXEC) },
            0
        );
        validate_remote(&remote).unwrap();
        assert_ne!(
            unsafe { libc::fcntl(remote.as_raw_fd(), libc::F_GETFD) } & libc::FD_CLOEXEC,
            0
        );
        let file: OwnedFd = std::fs::File::open("/dev/null").unwrap().into();
        assert!(validate_remote(&file).is_err());
        let (socket, _peer) = std::os::unix::net::UnixDatagram::pair().unwrap();
        let socket: OwnedFd = socket.into();
        assert!(validate_remote(&socket).is_err());
    }
    #[test]
    fn native_buffer_descriptors_and_transform_are_bounded_before_reading_pixels() {
        let mut pixels = [0u8; 16];
        let mut chunk: spa::sys::spa_chunk = unsafe { std::mem::zeroed() };
        let mut data: spa::sys::spa_data = unsafe { std::mem::zeroed() };
        data.type_ = spa::sys::SPA_DATA_MemPtr;
        data.maxsize = 16;
        data.data = pixels.as_mut_ptr().cast();
        data.chunk = &mut chunk;
        let mut raw: spa::sys::spa_buffer = unsafe { std::mem::zeroed() };
        raw.n_datas = 1;
        raw.datas = &mut data;
        let mut buffer: pw::sys::pw_buffer = unsafe { std::mem::zeroed() };
        buffer.buffer = &mut raw;
        assert_eq!(unsafe { inspect_buffer(&mut buffer) }.unwrap(), 16);
        raw.n_datas = 2;
        assert!(unsafe { inspect_buffer(&mut buffer) }.is_err());
        raw.n_datas = 1;
        data.type_ = spa::sys::SPA_DATA_DmaBuf;
        assert!(unsafe { inspect_buffer(&mut buffer) }.is_err());
        data.type_ = spa::sys::SPA_DATA_MemPtr;
        data.maxsize = (pixels::MAX_MAPPED_BYTES + 1) as u32;
        assert!(unsafe { inspect_buffer(&mut buffer) }.is_err());
        data.maxsize = 16;
        let mut transform = 0u32;
        let mut meta: spa::sys::spa_meta = unsafe { std::mem::zeroed() };
        meta.type_ = spa::sys::SPA_META_VideoTransform;
        meta.size = 4;
        meta.data = (&mut transform as *mut u32).cast();
        raw.n_metas = 1;
        raw.metas = &mut meta;
        assert!(unsafe { inspect_buffer(&mut buffer) }.is_ok());
        transform = 1;
        assert!(unsafe { inspect_buffer(&mut buffer) }.is_err());
        let _ = transform;
        meta.size = 3;
        assert!(unsafe { inspect_buffer(&mut buffer) }.is_err());
        raw.n_metas = 17;
        assert!(unsafe { inspect_buffer(&mut buffer) }.is_err());
    }
}

#[cfg(test)]
mod freshness_tests {
    use super::*;
    #[test]
    fn queued_monotonic_old_pts_never_establish_source_freshness() {
        let mut clock = VideoClock::default();
        assert!(clock.admit(Some(100), 100).unwrap());
        clock.start = Some(VideoStart { cutoff_ns: 150 });
        for pts in 101..=150 {
            assert!(!clock.admit(Some(pts), 200).unwrap());
        }
        assert!(!clock.fresh);
        assert!(clock.admit(Some(151), 200).unwrap());
        assert!(clock.fresh);
        assert!(clock.admit(Some(150), 200).is_err());
    }
    #[test]
    fn missing_unknown_future_and_regressing_pts_fail_closed() {
        for pts in [None, Some(i64::MIN), Some(-1), Some(0), Some(201)] {
            assert!(VideoClock::default().admit(pts, 200).is_err());
        }
        let mut clock = VideoClock::default();
        assert!(clock.admit(Some(100), 200).unwrap());
        assert!(clock.admit(Some(100), 200).is_err());
        assert!(clock.admit(Some(99), 200).is_err());
    }
}
