//! Default Kit controls plus the small capture/editor icon vocabulary.
use gpui_kit::{AssetSource, SharedString, assets};
use std::borrow::Cow;

assets::icon_assets!(
    CaptureIcons,
    [
        Camera,
        Crop,
        Timer,
        Monitor,
        Upload,
        Library,
        Image,
        Images,
        LogOut,
        Pencil,
        ArrowUpRight,
        Circle,
        MousePointer,
        Type,
        ListOrdered,
        Highlighter,
        Focus,
        Shield,
        Grid2x2,
        Droplet,
    ]
);
pub(super) struct AppAssets;
impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        if let Some(bytes) = CaptureIcons.load(path)? {
            return Ok(Some(bytes));
        }
        assets::Assets.load(path)
    }
    fn list(&self, path: &str) -> anyhow::Result<Vec<SharedString>> {
        let mut paths = assets::Assets.list(path)?;
        paths.extend(CaptureIcons.list(path)?);
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{DevicePixels, SvgRenderer, SvgSize, size};
    use std::sync::Arc;

    #[test]
    fn maintained_svg_renderer_draws_the_embedded_application_icons() {
        let renderer = SvgRenderer::new(Arc::new(AppAssets));
        let paths = CaptureIcons.list("icons/").unwrap();
        assert!(paths.len() >= 20);
        for path in paths {
            let svg = CaptureIcons.load(&path).unwrap().unwrap();
            let parsed = renderer.parse_svg(&svg).unwrap();
            let image = renderer
                .render_parsed(
                    &parsed,
                    SvgSize::ExactSize(size(DevicePixels(24), DevicePixels(24))),
                )
                .unwrap();
            assert_eq!(image.size(0), size(DevicePixels(24), DevicePixels(24)));
            assert!(
                image
                    .as_bytes(0)
                    .unwrap()
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .any(|p| p[3] != 0),
                "Embedded icon rendered empty: {path}"
            );
        }
    }

    struct TestFonts;
    impl AssetSource for TestFonts {
        fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
            let data: &'static [u8] = match path {
                "fonts/ibm-plex-sans/IBMPlexSans-Regular.ttf" => {
                    include_bytes!("../../../vendor/cosmic-text/fonts/NotoSans-Regular.ttf")
                }
                "fonts/lilex/Lilex-Regular.ttf" => {
                    include_bytes!("../../../vendor/cosmic-text/fonts/NotoSansArabic.ttf")
                }
                _ => return Ok(None),
            };
            Ok(Some(Cow::Borrowed(data)))
        }
        fn list(&self, _: &str) -> anyhow::Result<Vec<SharedString>> {
            Ok(Vec::new())
        }
    }

    #[test]
    fn maintained_svg_renderer_shapes_text_and_combining_marks_with_supplied_fonts() {
        let renderer = SvgRenderer::new(Arc::new(TestFonts));
        for text in ["Capture e\u{301}\u{302}\u{303}", "مرحبا", "Capture 😀 ✅"] {
            let svg = format!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="240" height="64"><text x="8" y="40" font-family="Noto Sans, Noto Sans Arabic" font-size="26">{text}</text></svg>"#
            );
            let parsed = renderer.parse_svg(svg.as_bytes()).unwrap();
            let image = renderer
                .render_parsed(&parsed, SvgSize::ScaleFactor(1.0))
                .unwrap();
            assert!(
                image
                    .as_bytes(0)
                    .unwrap()
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .filter(|p| p[3] != 0)
                    .count()
                    > 40,
                "Text rendered empty: {text}"
            );
        }
    }
}
