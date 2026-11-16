//! Synthetic template admission through the real helper error and private wire seams.
use crate::{
    CaptureError, CaptureMonitorBinding, CaptureOptions, CaptureRegionTemplate,
    CaptureTemplateGeometry, CapturedImage, DisplayInfo, MacDisplayCoordinates, MacDisplayProof,
    MacLogicalRect, NativeCaptureCall, NativeCaptureReply, PixelFormat,
};
use image::{Rgba, RgbaImage};
use std::{path::PathBuf, sync::Arc};

// The desktop transport consumes video constants that these pure wire tests do not use.
#[allow(dead_code)]
#[path = "../../../apps/desktop/src/capture_worker_protocol.rs"]
mod protocol;

fn observed(scale: f64) -> DisplayInfo {
    let logical = MacLogicalRect {
        x: -100.0,
        y: 0.0,
        width: 100.0,
        height: 80.0,
    };
    let proof = MacDisplayProof {
        id: 7,
        uuid: "00000000-0000-0000-0000-000000000001".into(),
        logical,
        pixel_width: (100.0 * scale) as u32,
        pixel_height: (80.0 * scale) as u32,
        rotation: 0.0,
    };
    let mac = MacDisplayCoordinates::from_filter(proof, logical, logical, scale).unwrap();
    DisplayInfo {
        id: "7".into(),
        name: "private synthetic".into(),
        x: -100,
        y: 0,
        width: mac.pixel_width,
        height: mac.pixel_height,
        scale_factor: scale as f32,
        rotation: 0.0,
        is_primary: true,
        mac_coordinates: Some(mac),
    }
}

fn template(width: u32, height: u32, scale: f32) -> CaptureRegionTemplate {
    CaptureRegionTemplate {
        name: "private legacy".into(),
        monitor: CaptureMonitorBinding::Physical {
            key: "macos:00000000-0000-0000-0000-000000000001".into(),
        },
        geometry: CaptureTemplateGeometry {
            width,
            height,
            scale_factor: scale,
            rotation: 0.0,
            mac_coordinates: None,
        },
        x: 1,
        y: 1,
        width: 1,
        height: 1,
        profile: None,
        hotkey: None,
    }
}

#[test]
fn legacy_retina_rebind_survives_helper_translation_and_private_wire_without_payload() {
    let display = observed(2.0);
    // Both old point-sized and numerically native-sized legacy records remain ambiguous.
    for template in [template(100, 80, 2.0), template(200, 160, 2.0)] {
        let call = NativeCaptureCall::Template {
            template: template.clone(),
            options: CaptureOptions::default(),
        };
        call.validate().unwrap();
        let error = template
            .geometry
            .admit_observed_display(&template.monitor, &display)
            .unwrap_err();
        assert!(matches!(error, CaptureError::TemplateRebindRequired));
        let error = crate::native_capture_job::template_operation_error(
            anyhow::Error::new(error).context("private binding path and native display identity"),
        );
        let mut wire = Vec::new();
        protocol::failure(&mut wire, &error).unwrap();
        assert_eq!(wire.len(), 33);
        assert_eq!(wire[32], 17);
        let error = protocol::read_reply(&mut wire.as_slice(), &call)
            .err()
            .unwrap();
        assert!(matches!(
            error.downcast_ref::<CaptureError>(),
            Some(CaptureError::TemplateRebindRequired)
        ));
        assert!(error.to_string().contains("Explicitly rebind"));
        assert!(!wire.windows(7).any(|bytes| bytes == b"private"));
    }
}

#[test]
fn legacy_exact_one_to_one_geometry_and_bound_uuid_admit_a_crop_reply() {
    let display = observed(1.0);
    let template = template(100, 80, 1.0);
    template
        .geometry
        .admit_observed_display(&template.monitor, &display)
        .unwrap();
    let call = NativeCaptureCall::Template {
        template: template.clone(),
        options: CaptureOptions::default(),
    };
    call.validate().unwrap();
    let image = CapturedImage {
        display_id: display.id,
        window_id: None,
        width: 1,
        height: 1,
        png_path: PathBuf::new(),
        pixels: Arc::new(RgbaImage::from_pixel(1, 1, Rgba([9, 19, 29, 255]))),
        elapsed_ms: 0,
        source_format: PixelFormat::Rgba8UnormSrgb,
        display_geometry: None,
        desktop_layout: None,
    };
    let mut wire = Vec::new();
    protocol::reply(&mut wire, NativeCaptureReply::Frames(vec![image])).unwrap();
    let NativeCaptureReply::Frames(frames) =
        protocol::read_reply(&mut wire.as_slice(), &call).unwrap()
    else {
        panic!("expected crop frame")
    };
    assert_eq!(frames[0].pixels.get_pixel(0, 0).0, [9, 19, 29, 255]);
    let other_binding = CaptureMonitorBinding::Physical {
        key: "macos:00000000-0000-0000-0000-000000000002".into(),
    };
    assert!(matches!(
        template
            .geometry
            .admit_observed_display(&other_binding, &observed(1.0)),
        Err(CaptureError::TemplateRebindRequired)
    ));
}

#[test]
fn helper_wire_keeps_stale_permissions_colour_and_rebind_categories_distinct() {
    let call = NativeCaptureCall::Template {
        template: template(100, 80, 1.0),
        options: Default::default(),
    };
    for (error, code) in [
        (CaptureError::StaleRegion, 1),
        (CaptureError::MacScreenPermissionDenied, 13),
        (CaptureError::MacAccessibilityPermissionDenied, 14),
        (CaptureError::MacMicrophonePermissionDenied, 15),
        (CaptureError::ImageColorPolicy, 16),
        (CaptureError::TemplateRebindRequired, 17),
    ] {
        let expected = error.to_string();
        let error = crate::native_capture_job::template_operation_error(error.into());
        let mut wire = Vec::new();
        protocol::failure(&mut wire, &error).unwrap();
        assert_eq!(wire.len(), 33);
        assert_eq!(wire[32], code);
        assert_eq!(
            protocol::read_reply(&mut wire.as_slice(), &call)
                .err()
                .unwrap()
                .to_string(),
            expected
        );
    }
}
