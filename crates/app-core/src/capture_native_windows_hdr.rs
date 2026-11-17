//! One finite WGC scRGB still capture. No shared D3D state or resident frame pool.
use super::{DpiScope, wide};
use crate::PixelRegion;
use anyhow::{Context, Result, ensure};
use std::{mem::size_of, ptr, sync::mpsc::sync_channel, time::Duration};
use windows::{
    Foundation::TypedEventHandler,
    Graphics::{
        Capture::{
            Direct3D11CaptureFrame, Direct3D11CaptureFramePool, GraphicsCaptureItem,
            GraphicsCaptureSession,
        },
        DirectX::{Direct3D11::IDirect3DDevice, DirectXPixelFormat},
    },
    Win32::{
        Foundation::{HMODULE, HWND},
        Graphics::{
            Direct3D::D3D_DRIVER_TYPE_HARDWARE,
            Direct3D11::*,
            Dxgi::{Common::*, *},
            Gdi::HMONITOR,
        },
        System::WinRT::{
            Direct3D11::{CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess},
            Graphics::Capture::IGraphicsCaptureItemInterop,
            RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize,
        },
    },
    core::{IInspectable, Interface, factory},
};
use windows_sys::Win32::{
    Devices::Display::*,
    Foundation::POINT,
    Graphics::Gdi::{MONITOR_DEFAULTTONULL, MonitorFromPoint},
};

pub(super) use crate::capture_windows_admission::{Color, WindowColor};

fn proven_legacy_query(status: i32) -> Result<bool> {
    use windows_sys::Win32::System::{
        LibraryLoader::{GetModuleHandleW, GetProcAddress},
        SystemInformation::{GetVersionExW, OSVERSIONINFOW},
    };
    if !matches!(status, 50 | 87) {
        return Ok(false);
    }
    let module = unsafe { GetModuleHandleW(windows::core::w!("user32.dll").as_ptr()) };
    ensure!(
        !module.is_null(),
        "Cannot prove native legacy OS color capability"
    );
    // This public entry point is mandatory from Win10 1703 (the HDR boundary).
    // Its presence prevents manifest-virtualized GetVersionEx from claiming an
    // older OS on a modern desktop. Neither API changes native DPI state.
    let modern_api =
        unsafe { GetProcAddress(module, c"SetProcessDpiAwarenessContext".as_ptr().cast()) }
            .is_some();
    let mut info = OSVERSIONINFOW {
        dwOSVersionInfoSize: size_of::<OSVERSIONINFOW>() as u32,
        ..Default::default()
    };
    ensure!(
        unsafe { GetVersionExW(&mut info) } != 0,
        "Cannot prove native legacy OS color capability"
    );
    Ok(crate::capture_windows_admission::legacy_query_unavailable(
        status,
        info.dwMajorVersion,
        info.dwBuildNumber,
        modern_api,
    ))
}

/// Advanced Color is queried for the actual active target, never inferred from
/// channel depth, a monitor name or the presence of an HDR-capable panel.
pub(super) fn color(monitor: &xcap::Monitor) -> Result<Option<Color>> {
    let mut paths_len = 0;
    let mut modes_len = 0;
    ensure!(
        unsafe {
            GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut paths_len, &mut modes_len)
        } == 0
            && (1..=64).contains(&paths_len)
            && modes_len <= 256,
        "Cannot admit active native color topology"
    );
    let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); paths_len as usize];
    let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); modes_len as usize];
    let capacity = (paths_len, modes_len);
    ensure!(
        unsafe {
            QueryDisplayConfig(
                QDC_ONLY_ACTIVE_PATHS,
                &mut paths_len,
                paths.as_mut_ptr(),
                &mut modes_len,
                modes.as_mut_ptr(),
                ptr::null_mut(),
            )
        } == 0
            && paths_len <= capacity.0
            && modes_len <= capacity.1,
        "Native color topology changed during admission"
    );
    let name = monitor.name()?;
    let mut matches = Vec::new();
    for path in paths.iter().take(paths_len as usize) {
        let mut source = DISPLAYCONFIG_SOURCE_DEVICE_NAME::default();
        source.header = DISPLAYCONFIG_DEVICE_INFO_HEADER {
            r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
            size: size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32,
            adapterId: path.sourceInfo.adapterId,
            id: path.sourceInfo.id,
        };
        ensure!(
            unsafe { DisplayConfigGetDeviceInfo(&mut source.header) } == 0,
            "Cannot bind native color source"
        );
        if !wide(&source.viewGdiDeviceName)?.eq_ignore_ascii_case(&name) {
            continue;
        }

        let mut info = DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO::default();
        info.header = DISPLAYCONFIG_DEVICE_INFO_HEADER {
            r#type: DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO,
            size: size_of::<DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO>() as u32,
            adapterId: path.targetInfo.adapterId,
            id: path.targetInfo.id,
        };
        let status = unsafe { DisplayConfigGetDeviceInfo(&mut info.header) };
        // This query predates Windows HDR. Preserve the prior SDR capture path
        // on the original supported Win10 versions without that API.
        if proven_legacy_query(status)? {
            matches.push(None);
            continue;
        }
        ensure!(status == 0, "Cannot determine native Advanced Color state");
        let flags = unsafe { info.Anonymous.value };
        if flags & 2 == 0 {
            matches.push(None);
            continue;
        }
        let output_space = output_color_space(&name)?;
        let hdr = output_space == DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020.0;
        ensure!(
            hdr || output_space == DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709.0
                || output_space == DXGI_COLOR_SPACE_RGB_FULL_G10_NONE_P709.0,
            "Unsupported native desktop output color space"
        );
        let mut white = DISPLAYCONFIG_SDR_WHITE_LEVEL::default();
        white.header = DISPLAYCONFIG_DEVICE_INFO_HEADER {
            r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SDR_WHITE_LEVEL,
            size: size_of::<DISPLAYCONFIG_SDR_WHITE_LEVEL>() as u32,
            adapterId: path.targetInfo.adapterId,
            id: path.targetInfo.id,
        };
        if hdr {
            ensure!(
                unsafe { DisplayConfigGetDeviceInfo(&mut white.header) } == 0
                    && (1000..=100_000).contains(&white.SDRWhiteLevel),
                "Native HDR reference white is unavailable"
            );
        }
        matches.push(Some(Color {
            hdr,
            white_level: if hdr { white.SDRWhiteLevel } else { 1000 },
            adapter_low: path.targetInfo.adapterId.LowPart,
            adapter_high: path.targetInfo.adapterId.HighPart,
            target: path.targetInfo.id,
            flags,
            encoding: info.colorEncoding,
            bits: info.bitsPerColorChannel,
            output_space,
        }));
    }
    crate::capture_windows_admission::matching_color(&matches).map_err(anyhow::Error::from)
}

fn output_color_space(name: &str) -> Result<i32> {
    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1()? };
    let mut outputs = 0;
    for adapter_index in 0..16 {
        let adapter = match unsafe { factory.EnumAdapters1(adapter_index) } {
            Ok(value) => value,
            Err(error) if error.code() == DXGI_ERROR_NOT_FOUND => {
                return Err(anyhow::anyhow!("Native Advanced Color output disappeared"));
            }
            Err(error) => return Err(error.into()),
        };
        for output_index in 0..32 {
            let output = match unsafe { adapter.EnumOutputs(output_index) } {
                Ok(value) => value,
                Err(error) if error.code() == DXGI_ERROR_NOT_FOUND => break,
                Err(error) => return Err(error.into()),
            };
            outputs += 1;
            ensure!(
                outputs <= 64,
                "Native color output inventory exceeds its bound"
            );
            let desc = unsafe { output.GetDesc()? };
            if wide(&desc.DeviceName)?.eq_ignore_ascii_case(name) {
                let output: IDXGIOutput6 = output
                    .cast()
                    .context("Native Advanced Color descriptor unavailable")?;
                return Ok(unsafe { output.GetDesc1()? }.ColorSpace.0);
            }
        }
    }
    anyhow::bail!("Native color adapter inventory exceeds its bound")
}

struct Apartment;
impl Apartment {
    fn new() -> Result<Self> {
        unsafe {
            RoInitialize(RO_INIT_MULTITHREADED)?;
        }
        Ok(Self)
    }
}
impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe {
            RoUninitialize();
        }
    }
}
struct Session {
    session: windows::Graphics::Capture::GraphicsCaptureSession,
    pool: Direct3D11CaptureFramePool,
    event: i64,
}
impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.session.Close();
        let _ = self.pool.RemoveFrameArrived(self.event);
        let _ = self.pool.Close();
    }
}
struct Frame(Direct3D11CaptureFrame);
impl Drop for Frame {
    fn drop(&mut self) {
        let _ = self.0.Close();
    }
}
struct Mapping<'a> {
    context: &'a ID3D11DeviceContext,
    resource: ID3D11Resource,
}
impl Drop for Mapping<'_> {
    fn drop(&mut self) {
        unsafe {
            self.context.Unmap(&self.resource, 0);
        }
    }
}

fn capture(
    item: GraphicsCaptureItem,
    area: PixelRegion,
    state: Color,
    check: &impl Fn() -> Result<()>,
) -> Result<image::RgbaImage> {
    ensure!(
        GraphicsCaptureSession::IsSupported()?,
        "Native float screen capture is unavailable"
    );
    crate::validate_image_size(area.width, area.height)?;
    let size = item.Size()?;
    ensure!(
        size.Width > 0 && size.Height > 0,
        "Native float capture has no physical extent"
    );
    let (width, height) = (size.Width as u32, size.Height as u32);
    let source_bytes = u64::from(width) * u64::from(height) * 8;
    ensure!(
        source_bytes <= crate::MAX_CAPTURE_BYTES
            && area
                .x
                .checked_add(area.width)
                .is_some_and(|end| end <= width)
            && area
                .y
                .checked_add(area.height)
                .is_some_and(|end| end <= height),
        "Native float source exceeds memory/geometry admission"
    );
    // One frame texture + one FP16 staging texture count against the GPU image
    // budget S+E+32 MiB. A 4K RGBA8 output is below the 32 MiB staging allowance.
    ensure!(
        u64::from(area.width) * u64::from(area.height) * 4 <= 32 * 1024 * 1024,
        "Native float staging exceeds active GPU-image admission; choose a smaller region"
    );
    let mut device = None;
    let mut context = None;
    unsafe {
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )?;
    }
    let device = device.context("Native float device unavailable")?;
    let context = context.context("Native float device context unavailable")?;
    let support = unsafe { device.CheckFormatSupport(DXGI_FORMAT_R16G16B16A16_FLOAT)? };
    ensure!(
        support & D3D11_FORMAT_SUPPORT_TEXTURE2D.0 as u32 != 0,
        "Native float texture format unavailable"
    );
    let dxgi: IDXGIDevice = device.cast()?;
    let direct: IDirect3DDevice = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi)? }.cast()?;
    check()?;
    let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
        &direct,
        DirectXPixelFormat::R16G16B16A16Float,
        1,
        size,
    )?;
    let (sender, receiver) = sync_channel(1);
    let event = match pool.FrameArrived(&TypedEventHandler::<
        Direct3D11CaptureFramePool,
        IInspectable,
    >::new(move |_, _| {
        let _ = sender.try_send(());
        Ok(())
    })) {
        Ok(event) => event,
        Err(error) => {
            let _ = pool.Close();
            return Err(error.into());
        }
    };
    let session = match pool.CreateCaptureSession(&item) {
        Ok(session) => session,
        Err(error) => {
            let _ = pool.RemoveFrameArrived(event);
            let _ = pool.Close();
            return Err(error.into());
        }
    };
    let owner = Session {
        session,
        pool,
        event,
    };
    // Cursor is composed exactly once by the established capture policy.
    owner
        .session
        .SetIsCursorCaptureEnabled(false)
        .context("Native float cursor exclusion unavailable")?;
    check()?;
    owner.session.StartCapture()?;
    receiver
        .recv_timeout(Duration::from_secs(3))
        .context("Native float still capture deadline exceeded")?;
    let frame = Frame(owner.pool.TryGetNextFrame()?);
    let content = frame.0.ContentSize()?;
    ensure!(
        content.Width == size.Width && content.Height == size.Height,
        "Native float capture source resized during acquisition"
    );
    let surface = frame.0.Surface()?;
    let access: IDirect3DDxgiInterfaceAccess = surface.cast()?;
    let texture: ID3D11Texture2D = unsafe { access.GetInterface()? };
    let mut desc = D3D11_TEXTURE2D_DESC::default();
    unsafe {
        texture.GetDesc(&mut desc);
    }
    ensure!(
        desc.Width == width
            && desc.Height == height
            && desc.Format == DXGI_FORMAT_R16G16B16A16_FLOAT
            && desc.MipLevels == 1
            && desc.ArraySize == 1
            && desc.SampleDesc.Count == 1,
        "Native float surface format/extent changed"
    );
    // Stop the producer before mapping/conversion; the checked-out frame stays
    // alive until all GPU source reads finish. No second frame can be retained.
    owner.session.Close()?;
    check()?;
    let mut staging_desc = desc;
    staging_desc.Width = area.width;
    staging_desc.Height = area.height;
    staging_desc.BindFlags = 0;
    staging_desc.MiscFlags = 0;
    staging_desc.Usage = D3D11_USAGE_STAGING;
    staging_desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
    let mut staging = None;
    unsafe {
        device.CreateTexture2D(&staging_desc, None, Some(&mut staging))?;
    }
    let staging = staging.context("Native bounded float staging allocation unavailable")?;
    let resource: ID3D11Resource = staging.cast()?;
    let source: ID3D11Resource = texture.cast()?;
    let bounds = D3D11_BOX {
        left: area.x,
        top: area.y,
        right: area.x + area.width,
        bottom: area.y + area.height,
        front: 0,
        back: 1,
    };
    unsafe {
        context.CopySubresourceRegion(&resource, 0, 0, 0, 0, &source, 0, Some(&bounds));
    }
    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    unsafe {
        context.Map(&resource, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
    }
    let _mapping = Mapping {
        context: &context,
        resource,
    };
    let stride = mapped.RowPitch as usize;
    let extent = stride
        .checked_mul(area.height as usize)
        .context("Native float row extent overflow")?;
    ensure!(
        !mapped.pData.is_null()
            && stride >= area.width as usize * 8
            && extent <= crate::MAX_CAPTURE_BYTES as usize,
        "Native float staging exceeds source admission"
    );
    let bytes = unsafe { std::slice::from_raw_parts(mapped.pData.cast::<u8>(), extent) };
    let output = crate::capture_color::half_rgba_to_sdr(
        area.width,
        area.height,
        stride,
        bytes,
        state.hdr,
        state.white_level,
        &|| false,
    )?;
    unsafe {
        device.GetDeviceRemovedReason()?;
    }
    drop(_mapping);
    drop(staging);
    drop(source);
    drop(texture);
    drop(access);
    drop(surface);
    drop(frame);
    drop(owner);
    Ok(output)
}

pub(super) fn display(
    monitor: &xcap::Monitor,
    area: PixelRegion,
    before: Color,
    check: &impl Fn() -> Result<()>,
) -> Result<image::RgbaImage> {
    let _dpi = DpiScope::new()?;
    let _apartment = Apartment::new()?;
    let geometry = super::monitor_geometry(monitor)?;
    let center = POINT {
        x: i32::try_from(i64::from(geometry.x) + i64::from(geometry.width / 2))
            .context("Native float monitor center overflow")?,
        y: i32::try_from(i64::from(geometry.y) + i64::from(geometry.height / 2))
            .context("Native float monitor center overflow")?,
    };
    let handle = unsafe { MonitorFromPoint(center, MONITOR_DEFAULTTONULL) };
    ensure!(!handle.is_null(), "Native float monitor disappeared");
    let interop = factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
    let item = unsafe { interop.CreateForMonitor::<GraphicsCaptureItem>(HMONITOR(handle))? };
    let size = item.Size()?;
    ensure!(
        size.Width == geometry.width as i32 && size.Height == geometry.height as i32,
        "Native float monitor pixel calibration changed"
    );
    let output = capture(item, area, before, check)?;
    ensure!(
        color(monitor)? == Some(before) && super::monitor_geometry(monitor)? == geometry,
        "Native float display color/geometry changed during acquisition"
    );
    Ok(output)
}

pub(super) fn window(
    window: &xcap::Window,
    full: [i32; 4],
    before: Color,
    check: &impl Fn() -> Result<()>,
) -> Result<image::RgbaImage> {
    let _apartment = Apartment::new()?;
    let interop = factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
    let item = unsafe {
        interop.CreateForWindow::<GraphicsCaptureItem>(HWND(window.id()? as usize as *mut _))?
    };
    let (width, height) = (
        u32::try_from(i64::from(full[2]) - i64::from(full[0]))?,
        u32::try_from(i64::from(full[3]) - i64::from(full[1]))?,
    );
    let size = item.Size()?;
    ensure!(
        size.Width == width as i32 && size.Height == height as i32,
        "Native float window frame disagrees with physical capture bounds"
    );
    capture(
        item,
        PixelRegion {
            x: 0,
            y: 0,
            width,
            height,
        },
        before,
        check,
    )
}

pub(super) fn window_color(full: [i32; 4]) -> Result<WindowColor> {
    let width = u32::try_from(i64::from(full[2]) - i64::from(full[0]))?;
    let height = u32::try_from(i64::from(full[3]) - i64::from(full[1]))?;
    crate::validate_image_size(width, height)?;
    let expected = i64::from(width) * i64::from(height);
    let monitors = xcap::Monitor::all()?;
    ensure!(
        monitors.len() <= 32,
        "Native window color inventory exceeds its bound"
    );
    let mut displays = Vec::new();
    let mut selected = None;
    let mut reference = None;
    let mut covered = 0i64;
    let mut has_hdr = false;
    for monitor in monitors {
        let geometry = super::monitor_geometry(&monitor)?;
        let left = i64::from(full[0]).max(i64::from(geometry.x));
        let top = i64::from(full[1]).max(i64::from(geometry.y));
        let right = i64::from(full[2]).min(i64::from(geometry.x) + i64::from(geometry.width));
        let bottom = i64::from(full[3]).min(i64::from(geometry.y) + i64::from(geometry.height));
        if left >= right || top >= bottom {
            continue;
        }
        let state = color(&monitor)?;
        let white = state.map_or(1000, |color| color.white_level);
        if let Some(state) = state {
            has_hdr |= state.hdr;
            selected.get_or_insert(state);
        }
        if let Some(previous) = reference {
            ensure!(
                previous == white,
                "Window spans incompatible HDR reference whites; capture a display region"
            );
        }
        reference = Some(white);
        covered += (right - left) * (bottom - top);
        displays.push((monitor.name()?, geometry, state));
    }
    if let Some(color) = &mut selected {
        ensure!(
            covered == expected,
            "Native HDR window reference-white coverage is ambiguous"
        );
        color.hdr = has_hdr;
    }
    displays.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(WindowColor { selected, displays })
}
