//! Portable regressions for the ownership of a genuine derived desktop grid.
use crate::editor_document::{Affine, EditorDocument, Interpolation};
use crate::{CapturedImage, MacDesktopLayout, PixelRegion};
use image::{Rgba, RgbaImage};
use std::{path::PathBuf, sync::Arc};

fn display(id: u32, x: f64, scale: f64) -> crate::MacDisplayCoordinates {
    let logical = crate::MacLogicalRect {
        x,
        y: 0.0,
        width: 100.0,
        height: 80.0,
    };
    let proof = crate::MacDisplayProof {
        id,
        uuid: format!("00000000-0000-0000-0000-{id:012x}"),
        logical,
        pixel_width: (100.0 * scale) as u32,
        pixel_height: (80.0 * scale) as u32,
        rotation: 0.0,
    };
    crate::MacDisplayCoordinates::from_filter(proof, logical, logical, scale).unwrap()
}

fn desktop() -> CapturedImage {
    let displays = [display(1, 0.0, 1.0), display(2, 100.0, 2.0)];
    let layout = MacDesktopLayout::new(&displays).unwrap();
    let frames = displays
        .iter()
        .map(|display| CapturedImage {
            display_id: display.display_id.to_string(),
            window_id: None,
            width: display.pixel_width,
            height: display.pixel_height,
            png_path: PathBuf::new(),
            pixels: Arc::new(RgbaImage::from_fn(
                display.pixel_width,
                display.pixel_height,
                |x, y| Rgba([x as u8, y as u8, 37, 255]),
            )),
            elapsed_ms: 0,
            source_format: crate::PixelFormat::Rgba8UnormSrgb,
            display_geometry: None,
            desktop_layout: None,
        })
        .collect::<Vec<_>>();
    let mut output = crate::stitch_macos_desktop_with_cancel(&layout, &frames, &|| false).unwrap();
    output.desktop_layout = Some(Arc::new(layout));
    assert_eq!(output.pixels.dimensions(), (400, 160));
    output
}

fn assert_edited(frame: &CapturedImage) {
    assert_eq!(frame.pixels.dimensions(), (frame.width, frame.height));
    assert!(frame.display_geometry.is_none());
    assert!(frame.desktop_layout.is_none());
}

#[test]
fn mac_desktop_metadata_is_removed_by_crop_and_same_size_pixel_edits() {
    let original = desktop();
    let original_pixels = original.pixels.clone();
    let original_layout = original.desktop_layout.clone().unwrap();
    let area = PixelRegion {
        x: 1,
        y: 1,
        width: 1,
        height: 1,
    };
    let cropped = crate::crop_snapshot(&original, area).unwrap();
    assert_eq!(cropped.pixels.dimensions(), (1, 1));
    assert_edited(&cropped);
    let painted = crate::redact_snapshot(&original, &[area]).unwrap();
    assert_eq!(painted.pixels.dimensions(), (400, 160));
    assert_ne!(
        painted.pixels.get_pixel(1, 1),
        original.pixels.get_pixel(1, 1)
    );
    assert_edited(&painted);
    assert!(Arc::ptr_eq(&original.pixels, &original_pixels));
    assert!(Arc::ptr_eq(
        original.desktop_layout.as_ref().unwrap(),
        &original_layout
    ));
    original_layout.validate().unwrap();
}

#[test]
fn mac_desktop_metadata_is_removed_by_document_crop_scale_rotation_flip_and_canvas_resize() {
    let original = desktop();
    let mut cropped = EditorDocument::from_image(original.clone()).unwrap();
    cropped
        .set_crop(Some(PixelRegion {
            x: 1,
            y: 1,
            width: 1,
            height: 1,
        }))
        .unwrap();
    assert_edited(&cropped.snapshot().render(|| false).unwrap());
    let mut scaled = EditorDocument::from_image(original.clone()).unwrap();
    scaled.scale_to(200, 80, Interpolation::Nearest).unwrap();
    assert_edited(&scaled.snapshot().render(|| false).unwrap());
    let mut rotated = EditorDocument::from_image(original.clone()).unwrap();
    rotated
        .transform_document(
            Affine::translate(160.0, 0.0).then(Affine::rotate(90.0, [0.0; 2])),
            [160, 400],
        )
        .unwrap();
    let rotated = rotated.snapshot().render(|| false).unwrap();
    assert_eq!(rotated.pixels.dimensions(), (160, 400));
    assert_edited(&rotated);
    let mut flipped = EditorDocument::from_image(original.clone()).unwrap();
    flipped
        .transform_document(
            Affine::translate(400.0, 0.0).then(Affine::scale(-1.0, 1.0)),
            [400, 160],
        )
        .unwrap();
    assert_edited(&flipped.snapshot().render(|| false).unwrap());
    let mut canvas = EditorDocument::from_image(original.clone()).unwrap();
    canvas.resize_canvas(402, 162, [0.5; 2]).unwrap();
    assert_edited(&canvas.snapshot().render(|| false).unwrap());
    assert_eq!(
        original.desktop_layout.as_ref().unwrap().width,
        original.width
    );
    assert_eq!(
        original.desktop_layout.as_ref().unwrap().height,
        original.height
    );
}

#[test]
fn mac_desktop_document_keeps_the_unchanged_original_but_never_attaches_it_to_a_new_raster() {
    let original = desktop();
    let pixels = original.pixels.clone();
    let layout = original.desktop_layout.clone().unwrap();
    let mut document = EditorDocument::from_image(original.clone()).unwrap();
    let identity = document.snapshot().render(|| false).unwrap();
    assert_eq!(identity.pixels, original.pixels);
    assert_edited(&identity);
    document.scale_to(200, 80, Interpolation::Nearest).unwrap();
    document.reset().unwrap();
    let reset = document.snapshot().render(|| false).unwrap();
    assert_eq!(reset.pixels, original.pixels);
    assert_edited(&reset);
    assert!(Arc::ptr_eq(&original.pixels, &pixels));
    assert!(Arc::ptr_eq(
        original.desktop_layout.as_ref().unwrap(),
        &layout
    ));
}
