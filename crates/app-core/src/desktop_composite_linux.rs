//! One operation-scoped RandR witness for root-raster desktop composition.
use crate::{CaptureError, DisplayInfo, NativeCaptureCheck};
use x11rb::{
    connection::Connection,
    protocol::{
        randr::{ConnectionExt as _, NotifyMask},
        xproto::ConnectionExt as _,
    },
    rust_connection::RustConnection,
};

type Result<T> = std::result::Result<T, CaptureError>;
fn native(error: impl std::fmt::Display) -> CaptureError {
    CaptureError::Backend(error.to_string())
}
fn unsupported() -> CaptureError {
    CaptureError::Unsupported(
        "X11 desktop composition requires one screen, complete nonmirrored output-backed RandR monitors, identity CRTC transforms and disabled panning",
    )
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Source {
    output: u32,
    crtc: u32,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    rotation: u16,
}
#[derive(Debug, PartialEq, Eq)]
struct Snapshot {
    timestamp: u32,
    config_timestamp: u32,
    monitor_timestamp: u32,
    width: u16,
    height: u16,
    sources: Vec<Source>,
}

pub(super) struct Guard {
    connection: RustConnection,
    root: u32,
    before: Snapshot,
}
impl Guard {
    pub(super) fn new(check: &NativeCaptureCheck<'_>) -> Result<Self> {
        check()?;
        let display = std::env::var("DISPLAY").map_err(native)?;
        if crate::linux_wayland_session() || display.trim().is_empty() || display.len() > 256 {
            return Err(unsupported());
        }
        let (connection, screen) = x11rb::connect(Some(&display)).map_err(native)?;
        if connection.setup().roots.len() != 1 {
            return Err(unsupported());
        }
        let version = connection
            .randr_query_version(1, 5)
            .map_err(native)?
            .reply()
            .map_err(native)?;
        if (version.major_version, version.minor_version) < (1, 5) {
            return Err(unsupported());
        }
        let root = connection.setup().roots[screen].root;
        connection
            .change_window_attributes(
                root,
                &x11rb::protocol::xproto::ChangeWindowAttributesAux::new()
                    .event_mask(x11rb::protocol::xproto::EventMask::STRUCTURE_NOTIFY),
            )
            .map_err(native)?
            .check()
            .map_err(native)?;
        connection
            .randr_select_input(
                root,
                NotifyMask::SCREEN_CHANGE
                    | NotifyMask::CRTC_CHANGE
                    | NotifyMask::OUTPUT_CHANGE
                    | NotifyMask::OUTPUT_PROPERTY
                    | NotifyMask::RESOURCE_CHANGE,
            )
            .map_err(native)?
            .check()
            .map_err(native)?;
        // Finish subscription and discard only startup notifications before the baseline.
        connection
            .get_input_focus()
            .map_err(native)?
            .reply()
            .map_err(native)?;
        for index in 0..256 {
            if connection.poll_for_event().map_err(native)?.is_none() {
                break;
            }
            if index == 255 {
                return Err(CaptureError::StaleRegion);
            }
        }
        let before = snapshot(&connection, root, check)?;
        let guard = Self {
            connection,
            root,
            before,
        };
        guard.check_events()?;
        Ok(guard)
    }
    pub(super) fn validate_displays(&self, displays: &[DisplayInfo]) -> Result<()> {
        validate_sources(&self.before.sources, displays)
    }
    pub(super) fn check_events(&self) -> Result<()> {
        if self.connection.poll_for_event().map_err(native)?.is_some() {
            return Err(CaptureError::StaleRegion);
        }
        Ok(())
    }
    pub(super) fn finish(&self, check: &NativeCaptureCheck<'_>) -> Result<()> {
        check()?;
        if self.before != snapshot(&self.connection, self.root, check)? {
            return Err(CaptureError::StaleRegion);
        }
        // The snapshot's round trips also deliver restored-layout events to this connection.
        self.check_events()?;
        check()
    }
}

fn snapshot(
    connection: &RustConnection,
    root: u32,
    check: &NativeCaptureCheck<'_>,
) -> Result<Snapshot> {
    check()?;
    let resources = connection
        .randr_get_screen_resources_current(root)
        .map_err(native)?
        .reply()
        .map_err(native)?;
    if resources.outputs.len() > 256 || resources.crtcs.len() > 256 {
        return Err(CaptureError::MemoryLimit);
    }
    let monitor_reply = connection
        .randr_get_monitors(root, true)
        .map_err(native)?
        .reply()
        .map_err(native)?;
    let monitor_timestamp = monitor_reply.timestamp;
    let monitors = monitor_reply.monitors;
    if monitors.is_empty() || monitors.len() > 32 {
        return Err(unsupported());
    }
    let geometry = connection
        .get_geometry(root)
        .map_err(native)?
        .reply()
        .map_err(native)?;
    let mut sources = Vec::with_capacity(monitors.len());
    for monitor in monitors {
        check()?;
        // Outputless logical monitor records must never disappear from a successful desktop.
        if monitor.outputs.len() != 1 {
            return Err(unsupported());
        }
        let output = monitor.outputs[0];
        let info = connection
            .randr_get_output_info(output, resources.config_timestamp)
            .map_err(native)?
            .reply()
            .map_err(native)?;
        if info.crtc == 0 || !resources.outputs.contains(&output) {
            return Err(CaptureError::StaleRegion);
        }
        let crtc = connection
            .randr_get_crtc_info(info.crtc, resources.config_timestamp)
            .map_err(native)?
            .reply()
            .map_err(native)?;
        let rotation = u16::from(crtc.rotation);
        if u8::from(info.status) != 0
            || u8::from(crtc.status) != 0
            || crtc.mode == 0
            || crtc.outputs != [output]
            || !matches!(rotation, 1 | 2 | 4 | 8)
            || (monitor.x, monitor.y, monitor.width, monitor.height)
                != (crtc.x, crtc.y, crtc.width, crtc.height)
        {
            return Err(unsupported());
        }
        let transform = connection
            .randr_get_crtc_transform(info.crtc)
            .map_err(native)?
            .reply()
            .map_err(native)?
            .current_transform;
        if [
            transform.matrix11,
            transform.matrix12,
            transform.matrix13,
            transform.matrix21,
            transform.matrix22,
            transform.matrix23,
            transform.matrix31,
            transform.matrix32,
            transform.matrix33,
        ] != [65536, 0, 0, 0, 65536, 0, 0, 0, 65536]
        {
            return Err(unsupported());
        }
        let panning = connection
            .randr_get_panning(info.crtc)
            .map_err(native)?
            .reply()
            .map_err(native)?;
        if u8::from(panning.status) != 0 || panning.width != 0 || panning.height != 0 {
            return Err(unsupported());
        }
        let source = Source {
            output,
            crtc: info.crtc,
            x: i32::from(monitor.x),
            y: i32::from(monitor.y),
            width: u32::from(monitor.width),
            height: u32::from(monitor.height),
            rotation,
        };
        if source.x < 0
            || source.y < 0
            || i64::from(source.x) + i64::from(source.width) > i64::from(geometry.width)
            || i64::from(source.y) + i64::from(source.height) > i64::from(geometry.height)
            || sources
                .iter()
                .any(|previous: &Source| previous.output == output || previous.crtc == info.crtc)
        {
            return Err(unsupported());
        }
        sources.push(source);
    }
    // An active CRTC excluded by custom monitor records is still part of the desktop.
    for crtc in &resources.crtcs {
        check()?;
        let info = connection
            .randr_get_crtc_info(*crtc, resources.config_timestamp)
            .map_err(native)?
            .reply()
            .map_err(native)?;
        if u8::from(info.status) != 0 {
            return Err(CaptureError::StaleRegion);
        }
        if info.mode != 0 && !sources.iter().any(|source| source.crtc == *crtc) {
            return Err(unsupported());
        }
    }
    sources.sort_unstable_by_key(|source| source.output);
    Ok(Snapshot {
        timestamp: resources.timestamp,
        config_timestamp: resources.config_timestamp,
        monitor_timestamp,
        width: geometry.width,
        height: geometry.height,
        sources,
    })
}
fn validate_sources(sources: &[Source], displays: &[DisplayInfo]) -> Result<()> {
    if sources.len() != displays.len()
        || displays.iter().any(|display| {
            !sources.iter().any(|source| {
                display.id == source.output.to_string()
                    && (display.x, display.y, display.width, display.height)
                        == (source.x, source.y, source.width, source.height)
                    && display.rotation
                        == match source.rotation {
                            1 => 0.0,
                            2 => 90.0,
                            4 => 180.0,
                            8 => 270.0,
                            _ => -1.0,
                        }
                    && display.mac_coordinates.is_none()
            })
        })
    {
        return Err(CaptureError::StaleRegion);
    }
    let mut ids = std::collections::HashSet::new();
    if displays.iter().any(|display| !ids.insert(&display.id)) {
        return Err(CaptureError::StaleRegion);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires scripts/test-linux-desktop-composite.py owned authenticated Xvfb"]
    fn private_x11_restored_monitor_change_invalidates_desktop() {
        use std::os::unix::fs::MetadataExt;
        assert_eq!(
            std::env::var("SCREENSHOTS_PRIVATE_DESKTOP").as_deref(),
            Ok("owned-96x64")
        );
        assert!(!crate::linux_wayland_session());
        let number: u16 = std::env::var("DISPLAY")
            .unwrap()
            .strip_prefix(':')
            .unwrap()
            .parse()
            .unwrap();
        assert!((380..440).contains(&number));
        let authority = std::path::PathBuf::from(std::env::var_os("XAUTHORITY").unwrap());
        let directory = authority.parent().unwrap();
        assert!(
            directory
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("screenshots-desktop-")
        );
        let metadata = std::fs::symlink_metadata(directory).unwrap();
        assert!(metadata.is_dir() && metadata.mode() & 0o077 == 0);
        let metadata = std::fs::symlink_metadata(&authority).unwrap();
        assert!(metadata.is_file() && metadata.uid() == unsafe { libc::getuid() });
        let guard = Guard::new(&|| Ok(())).unwrap();
        assert_eq!((guard.before.width, guard.before.height), (96, 64));
        guard.finish(&|| Ok(())).unwrap();
        let name = guard
            .connection
            .intern_atom(false, b"private-transient-desktop")
            .unwrap()
            .reply()
            .unwrap()
            .atom;
        guard
            .connection
            .randr_set_monitor(
                guard.root,
                x11rb::protocol::randr::MonitorInfo {
                    name,
                    primary: false,
                    automatic: false,
                    x: 0,
                    y: 0,
                    width: 1,
                    height: 1,
                    width_in_millimeters: 1,
                    height_in_millimeters: 1,
                    outputs: vec![],
                },
            )
            .unwrap()
            .check()
            .unwrap();
        guard
            .connection
            .randr_delete_monitor(guard.root, name)
            .unwrap()
            .check()
            .unwrap();
        assert!(matches!(
            guard.finish(&|| Ok(())),
            Err(CaptureError::StaleRegion)
        ));
    }

    #[test]
    fn native_output_provenance_rejects_omission_duplicate_rotation_and_logical_geometry() {
        let source = Source {
            output: 7,
            crtc: 3,
            x: 40,
            y: 0,
            width: 60,
            height: 80,
            rotation: 2,
        };
        let display = DisplayInfo {
            id: "7".into(),
            name: "private".into(),
            x: 40,
            y: 0,
            width: 60,
            height: 80,
            rotation: 90.0,
            scale_factor: 2.0,
            is_primary: true,
            mac_coordinates: None,
        };
        assert!(
            validate_sources(
                std::slice::from_ref(&source),
                std::slice::from_ref(&display)
            )
            .is_ok()
        );
        assert!(validate_sources(&[], std::slice::from_ref(&display)).is_err());
        for changed in 0..4 {
            let mut bad = display.clone();
            match changed {
                0 => bad.id = "8".into(),
                1 => bad.x /= 2,
                2 => bad.rotation = 0.0,
                _ => bad.width /= 2,
            }
            assert!(validate_sources(std::slice::from_ref(&source), &[bad]).is_err());
        }
        assert!(validate_sources(&[source.clone(), source], &[display.clone(), display]).is_err());
    }
}
