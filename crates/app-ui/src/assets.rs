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
