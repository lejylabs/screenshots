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

#[test]
fn sequential_metadata_errors_are_rejected_by_inspect_and_before_source_or_font_decode() {
    let fixture = Fixture::new();
    let initial = json!({"id":1,"bounds":{"x":1,"y":1,"width":2,"height":1},"content":{"tool":"draw","shape":{"shape":"rectangle"},"style":{"fill":[0,255,0,255],"stroke":null,"width":0}}});
    let invalid = vec![
        json!([{"action":"transform_document","transform":[1,0,0,1,0,0],"canvas":[0,0]}]),
        json!([{"action":"resize_canvas","width":8,"height":4,"anchor":[2,0]}]),
        json!([{"action":"remove","id":99}]),
        json!([{"action":"reorder","id":99,"index":0}]),
        json!([{"action":"tool_preset","id":99,"name":"green"}]),
        json!([{"action":"crop","area":{"x":7,"y":0,"width":2,"height":1}}]),
        json!([{"action":"remove","id":1},{"action":"transform_layer","id":1,"transform":[1,0,0,1,0,0]}]),
        json!([{"action":"duplicate","id":1,"new_id":1,"transform":[1,0,0,1,1,0]}]),
    ];
    for (index, edits) in invalid.into_iter().enumerate() {
        let mut scene = basic();
        scene["document"]["layers"] = json!([initial.clone()]);
        scene["fonts"] = json!([{"id":"unused","path":"missing.ttf"}]);
        scene["presets"] =
            json!({"version":1,"tools":[{"name":"green","content":initial["content"].clone()}]});
        scene["edits"] = edits;
        let path = fixture.scene(&format!("invalid-{index}.json"), scene);
        let inspected = invoke([std::ffi::OsStr::new("inspect"), path.as_os_str()]);
        assert!(!inspected.status.success(), "case{index}");
        let output = fixture.path(&format!("invalid-{index}.png"));
        let result = render(&fixture.path("missing.png"), &path, &output);
        assert!(!result.status.success());
        assert_eq!(
            result.stderr, inspected.stderr,
            "both commands must use the same sequential metadata validator"
        );
        assert!(!String::from_utf8_lossy(&result.stderr).contains("Could not open editor source"));
        assert!(!output.exists());
    }
}

#[test]
fn declared_source_size_drives_reset_preflight_and_matches_actual_oriented_input() {
    let fixture = Fixture::new();
    let input = fixture.path("wide.png");
    RgbaImage::from_pixel(16, 8, Rgba([3, 5, 7, 255]))
        .save(&input)
        .unwrap();
    let mut scene = basic();
    scene["source_size"] = json!([16, 8]);
    scene["edits"] =
        json!([{"action":"reset"},{"action":"crop","area":{"x":12,"y":4,"width":4,"height":4}}]);
    let profile = fixture.scene("declared.json", scene);
    let info = success(invoke([
        std::ffi::OsStr::new("inspect"),
        profile.as_os_str(),
    ]));
    assert_eq!(info["canvas"], json!([16, 8]));
    assert_eq!(info["source_size"], json!([16, 8]));
    let output = fixture.path("declared.png");
    let result = success(render(&input, &profile, &output));
    assert_eq!(result["width"], 4);
    assert_eq!(result["height"], 4);
    let small = fixture.source();
    let output = fixture.path("mismatch.png");
    assert!(!render(&small, &profile, &output).status.success());
    assert!(!output.exists());
}

#[test]
fn final_output_extent_is_refused_before_missing_source_or_asset_io() {
    let fixture = Fixture::new();
    for use_preset in [false, true] {
        let mut value = basic();
        value["document"]["canvas"] = json!([1, 1]);
        value["fonts"] = json!([{"id":"unused","path":"missing.ttf"}]);
        if use_preset {
            value["presets"] = json!({"version":1,"exports":[{"name":"huge","preset":{"padding":[8192,8192,8192,8192]}}]});
            value["export_preset"] = json!("huge");
            value["edits"] =
                json!([{"action":"resize_canvas","width":2,"height":2,"anchor":[0,0]}]);
        } else {
            value["document"]["export"] = json!({"padding":[8192,8192,8192,8192]});
        }
        let scene = fixture.scene(&format!("huge-{use_preset}.json"), value);
        let inspected = invoke([std::ffi::OsStr::new("inspect"), scene.as_os_str()]);
        assert!(!inspected.status.success());
        let destination = fixture.path(&format!("huge-{use_preset}.png"));
        let result = render(&fixture.path("missing.png"), &scene, &destination);
        assert!(!result.status.success());
        assert_eq!(
            result.stderr, inspected.stderr,
            "same final-output planner must run in both commands"
        );
        let error = String::from_utf8_lossy(&result.stderr);
        assert!(error.contains("256 MiB"), "{error}");
        assert!(!error.contains("Could not open editor source"));
        assert!(!error.contains("missing.ttf"));
        assert!(!destination.exists());
    }
}

#[test]
fn codec_output_limits_precede_missing_input_and_accept_exact_metadata_boundaries() {
    let fixture = Fixture::new();
    let refused = [
        ("webp_lossy", [4097, 4096], "webp", "64 MiB"),
        ("webp_lossy", [4096, 4097], "webp", "64 MiB"),
        ("webp_lossy", [16384, 1], "webp", "16383"),
        ("webp_lossy", [1, 16384], "webp", "16383"),
        ("webp", [20000, 1], "webp", "16384"),
        ("webp", [1, 16385], "webp", "16384"),
        ("jpeg", [65536, 1], "jpg", "65535"),
        ("jpeg", [1, 65536], "jpg", "65535"),
    ];
    for (index, (format, canvas, extension, expected)) in refused.into_iter().enumerate() {
        let mut value = basic();
        value["source_size"] = json!([1, 1]);
        value["document"]["canvas"] = json!(canvas);
        value["document"]["export"] = json!({"format":format});
        value["fonts"] = json!([{"id":"unused","path":"missing.ttf"}]);
        let scene = fixture.scene(&format!("codec-refused-{index}.json"), value);
        let inspection = invoke([std::ffi::OsStr::new("inspect"), scene.as_os_str()]);
        assert!(!inspection.status.success());
        let destination = fixture.path(&format!("codec-refused-{index}.{extension}"));
        let result = render(&fixture.path("missing.png"), &scene, &destination);
        assert!(!result.status.success());
        assert_eq!(result.stderr, inspection.stderr);
        let error = String::from_utf8_lossy(&result.stderr);
        assert!(error.contains(expected), "{error}");
        assert!(!error.contains("Could not open editor source"));
        assert!(!error.contains("missing.ttf"));
        assert!(!destination.exists());
    }
    for (index, (format, canvas)) in [
        ("webp_lossy", [4096, 4096]),
        ("webp_lossy", [16383, 1]),
        ("webp_lossy", [1, 16383]),
        ("webp", [16384, 1]),
        ("webp", [1, 16384]),
        ("jpeg", [65535, 1]),
        ("jpeg", [1, 65535]),
        ("png", [65536, 1]),
    ]
    .into_iter()
    .enumerate()
    {
        let mut value = basic();
        value["source_size"] = json!([1, 1]);
        value["document"]["canvas"] = json!(canvas);
        value["document"]["export"] = json!({"format":format});
        value["fonts"] = json!([{"id":"unused","path":"missing.ttf"}]);
        let scene = fixture.scene(&format!("codec-accepted-{index}.json"), value);
        let inspected = success(invoke([std::ffi::OsStr::new("inspect"), scene.as_os_str()]));
        assert_eq!(inspected["canvas"], json!(canvas));
    }
}
