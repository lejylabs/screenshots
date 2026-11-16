use image::{Rgba, RgbaImage};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    process::{Command, Output},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("screenshots-editor-cli-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
    fn scene(&self, name: &str, value: Value) -> PathBuf {
        let path = self.path(name);
        std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        path
    }
    fn source(&self) -> PathBuf {
        let path = self.path("source.png");
        RgbaImage::from_pixel(8, 4, Rgba([0, 0, 200, 255]))
            .save(&path)
            .unwrap();
        path
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn invoke(args: impl IntoIterator<Item = impl AsRef<std::ffi::OsStr>>) -> Output {
    Command::new(env!("CARGO_BIN_EXE_screenshots"))
        .arg("editor-document")
        .args(args)
        .output()
        .unwrap()
}
fn basic() -> Value {
    json!({"version":1,"document":{"version":1,"canvas":[8,4],"layers":[]}})
}
fn success(output: Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}
fn render(input: &Path, scene: &Path, output: &Path) -> Output {
    invoke([
        std::ffi::OsStr::new("render"),
        input.as_os_str(),
        scene.as_os_str(),
        output.as_os_str(),
    ])
}

#[test]
fn explicit_scene_publishes_final_pixels_and_never_overwrites() {
    let fixture = Fixture::new();
    let input = fixture.source();
    let original = std::fs::read(&input).unwrap();
    let mut value = basic();
    value["document"]["layers"] = json!([{"id":1,"bounds":{"x":1,"y":1,"width":3,"height":2},"content":{"tool":"draw","shape":{"shape":"rectangle"},"style":{"fill":[0,255,0,255],"stroke":null,"width":0}}}]);
    value["edits"] = json!([{"action":"transform_layer","id":1,"transform":[1,0,0,1,1,0]},{"action":"undo"},{"action":"redo"}]);
    let scene = fixture.scene("scene.json", value);
    let destination = fixture.path("output.png");
    let result = success(render(&input, &scene, &destination));
    assert_eq!(result["saved"], true);
    let pixels = image::open(&destination).unwrap().into_rgba8();
    assert_eq!(pixels.get_pixel(1, 1).0, [0, 0, 200, 255]);
    assert_eq!(pixels.get_pixel(2, 1).0, [0, 255, 0, 255]);
    assert_eq!(std::fs::read(&input).unwrap(), original);
    let first = std::fs::read(&destination).unwrap();
    assert!(!render(&input, &scene, &destination).status.success());
    assert_eq!(std::fs::read(destination).unwrap(), first);
}
#[test]
fn output_extension_conflict_precedes_missing_source_or_asset_io() {
    let fixture = Fixture::new();
    let mut value = basic();
    value["fonts"] = json!([{"id":"absent","path":"missing.ttf"}]);
    let scene = fixture.scene("scene.json", value);
    let output = render(
        &fixture.path("missing.png"),
        &scene,
        &fixture.path("wrong.jpg"),
    );
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("conflicts"));
    assert!(!fixture.path("wrong.jpg").exists());
}
#[test]
fn inspect_reports_counts_without_loading_assets_or_emitting_literal_text() {
    let fixture = Fixture::new();
    let mut value = basic();
    value["document"]["layers"] = json!([{"id":1,"bounds":{"x":0,"y":0,"width":128,"height":64},"content":{"tool":"text","text":{"spans":[{"text":"SYNTHETIC_PRIVATE_LABEL","style":{"fonts":["absent"],"size":16,"color":[255,255,255,255]}}]}}}]);
    value["fonts"] = json!([{"id":"absent","path":"does-not-exist.ttf"}]);
    let scene = fixture.scene("inspect.json", value);
    let output = invoke([std::ffi::OsStr::new("inspect"), scene.as_os_str()]);
    let result = success(output);
    assert_eq!(result["fonts"], 1);
    assert!(!result.to_string().contains("SYNTHETIC_PRIVATE_LABEL"));
}
#[test]
fn source_file_alias_cannot_be_reinserted_as_an_external_raster() {
    let fixture = Fixture::new();
    let input = fixture.source();
    let alias = fixture.path("alias.png");
    std::fs::hard_link(&input, &alias).unwrap();
    let mut value = basic();
    value["document"]["layers"] = json!([{"id":1,"bounds":{"x":0,"y":0,"width":4,"height":4},"content":{"tool":"raster","asset":"original","kind":"image"}}]);
    value["rasters"] = json!([{"id":"original","path":"alias.png"}]);
    let scene = fixture.scene("alias.json", value);
    let destination = fixture.path("refused.png");
    let output = render(&input, &scene, &destination);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("original source file"));
    assert!(!destination.exists());
}
