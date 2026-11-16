//! Registered as a child of upload so production extractors stay private.
use super::*;
use crate::{
    DisplayInfo, ExportSize, ImageExportFormat, PixelRegion, RegionPreset, RememberedRegion,
};
#[path = "../tests/support/structured_parser_mutation.rs"]
mod mutation;

const REGIONS: &str = include_str!("../tests/fixtures/structured_parser_fuzz/regions.jsonl");
const DRAGS: &str = include_str!("../tests/fixtures/structured_parser_fuzz/drag_bits.jsonl");
const NATIVE: &str = include_str!("../tests/fixtures/structured_parser_fuzz/native_uploads.jsonl");
const SHAREX: &str = include_str!("../tests/fixtures/structured_parser_fuzz/sharex_uploads.jsonl");
const RESPONSES: &str = include_str!("../tests/fixtures/structured_parser_fuzz/responses.jsonl");

fn lines(text: &str) -> Vec<Vec<u8>> {
    text.lines().map(|line| line.as_bytes().to_vec()).collect()
}

fn display() -> DisplayInfo {
    DisplayInfo {
        mac_coordinates: None,
        id: "fixture".into(),
        name: "Synthetic only".into(),
        x: -4096,
        y: -2160,
        width: 4096,
        height: 2160,
        scale_factor: 1.0,
        rotation: 0.0,
        is_primary: true,
    }
}

fn checked_area(area: PixelRegion, width: u32, height: u32) {
    assert!(area.width > 0 && area.height > 0);
    assert!(
        area.x
            .checked_add(area.width)
            .is_some_and(|end| end <= width)
    );
    assert!(
        area.y
            .checked_add(area.height)
            .is_some_and(|end| end <= height)
    );
}

fn geometry(bytes: &[u8]) -> usize {
    let parse = || serde_json::from_slice::<PixelRegion>(bytes).map_err(|error| error.to_string());
    let first = parse();
    assert!(first == parse(), "coordinate decoding changed");
    let Ok(area) = first else {
        return 0;
    };
    let preset = RegionPreset::new("Fixture", area);
    if let Ok(preset) = preset {
        assert!(area.x.checked_add(area.width).is_some());
        assert!(area.y.checked_add(area.height).is_some());
        assert!(crate::validate_image_size(area.width, area.height).is_ok());
        assert!(preset.area() == area);
    }
    let remember = || {
        RememberedRegion::from_display(&display(), area)
            .map(|region| region.area())
            .map_err(|error| error.to_string())
    };
    let remembered = remember();
    assert!(remembered == remember(), "region admission changed");
    if let Ok(region) = remembered {
        checked_area(region, 4096, 2160);
    }
    for size in [
        ExportSize::Original,
        ExportSize::Half,
        ExportSize::Quarter,
        ExportSize::Width(area.width),
        ExportSize::Height(area.height),
        ExportSize::Exact(area.width, area.height),
    ] {
        let dimensions = || {
            size.dimensions(4096, 2160)
                .map_err(|error| error.to_string())
        };
        let first = dimensions();
        assert!(first == dimensions(), "size admission changed");
        if let Ok((width, height)) = first {
            assert!((1..=4096).contains(&width) && (1..=2160).contains(&height));
            assert!(crate::validate_image_size(width, height).is_ok());
        }
    }
    serde_json::to_vec(&area).unwrap().len()
}

#[test]
fn structured_parser_fuzz_coordinates_and_sizes() {
    // Fixed answers prevent reject-all and always-original implementations passing properties.
    for (name, area) in [
        (
            " Fixture ",
            PixelRegion {
                x: 12,
                y: 24,
                width: 1920,
                height: 1080,
            },
        ),
        (
            "Fixture",
            PixelRegion {
                x: u32::MAX - 1,
                y: u32::MAX - 1,
                width: 1,
                height: 1,
            },
        ),
        (
            "Fixture",
            PixelRegion {
                x: 0,
                y: 0,
                width: 8192,
                height: 8192,
            },
        ),
    ] {
        let preset = RegionPreset::new(name, area).expect("valid fixed preset rejected");
        assert!(preset.name == "Fixture" && preset.area() == area);
    }
    let one = PixelRegion {
        x: 0,
        y: 0,
        width: 1,
        height: 1,
    };
    for name in ["", "\n", "Fixture\0", &"x".repeat(65)] {
        assert!(
            RegionPreset::new(name, one).is_err(),
            "invalid preset name accepted"
        );
    }
    for area in [
        PixelRegion { width: 0, ..one },
        PixelRegion { height: 0, ..one },
        PixelRegion { x: u32::MAX, ..one },
        PixelRegion { y: u32::MAX, ..one },
        PixelRegion {
            width: 8193,
            height: 8192,
            ..one
        },
        PixelRegion {
            width: u32::MAX,
            ..one
        },
    ] {
        assert!(
            RegionPreset::new("Fixture", area).is_err(),
            "invalid preset geometry accepted"
        );
        assert!(
            RememberedRegion::from_display(&display(), area).is_err(),
            "invalid remembered geometry accepted"
        );
    }
    for area in [
        PixelRegion { x: 4096, ..one },
        PixelRegion { y: 2160, ..one },
    ] {
        assert!(RememberedRegion::from_display(&display(), area).is_err());
    }
    for (width, height, accepted) in [
        (1, 1, true),
        (8192, 8192, true),
        (0, 1, false),
        (1, 0, false),
        (8193, 8192, false),
        (u32::MAX, 1, false),
        (1, u32::MAX, false),
    ] {
        assert!(
            crate::validate_image_size(width, height).is_ok() == accepted,
            "fixed RGBA admission answer changed"
        );
    }
    for (size, source, expected) in [
        (ExportSize::Original, (7, 5), (7, 5)),
        (ExportSize::Half, (7, 5), (4, 3)),
        (ExportSize::Quarter, (7, 5), (2, 2)),
        (ExportSize::Width(3), (7, 5), (3, 3)),
        (ExportSize::Height(2), (7, 5), (3, 2)),
        (ExportSize::Exact(3, 4), (7, 5), (3, 4)),
        (ExportSize::Half, (1, 7), (1, 4)),
        (ExportSize::Quarter, (7, 1), (2, 1)),
    ] {
        assert!(
            size.dimensions(source.0, source.1).unwrap() == expected,
            "fixed export dimensions changed"
        );
    }
    for size in [
        ExportSize::Width(0),
        ExportSize::Height(0),
        ExportSize::Exact(0, 1),
        ExportSize::Exact(1, 0),
        ExportSize::Width(8),
        ExportSize::Height(6),
        ExportSize::Exact(8, 1),
        ExportSize::Exact(1, 6),
        ExportSize::Width(u32::MAX),
    ] {
        assert!(
            size.dimensions(7, 5).is_err(),
            "invalid export size accepted"
        );
    }
    for size in [
        ExportSize::Original,
        ExportSize::Half,
        ExportSize::Quarter,
        ExportSize::Width(1),
        ExportSize::Height(1),
        ExportSize::Exact(1, 1),
    ] {
        for (width, height) in [(0, 1), (1, 0), (u32::MAX, 1)] {
            assert!(
                size.dimensions(width, height).is_err(),
                "invalid export source accepted"
            );
        }
    }
    let mut corpus = lines(REGIONS);
    for bytes in &corpus {
        let area: PixelRegion = serde_json::from_slice(bytes).unwrap();
        assert!(RegionPreset::new("Fixture", area).unwrap().area() == area);
        let remembered = RememberedRegion::from_display(&display(), area);
        if cfg!(target_os = "macos") {
            // A synthetic legacy display carries no independently observed native proof.
            assert!(
                remembered.is_err(),
                "macOS must reject absent native coordinate proof"
            );
        } else {
            assert!(remembered.unwrap().area() == area);
        }
    }
    for value in [0, 1, 8192, 65535, u32::MAX - 1, u32::MAX] {
        for field in ["x", "y", "width", "height"] {
            let mut area = serde_json::json!({"x":0,"y":0,"width":1,"height":1});
            area[field] = value.into();
            corpus.push(serde_json::to_vec(&area).unwrap());
        }
        let admission = crate::validate_image_size(value, value);
        if admission.is_ok() {
            assert!(value > 0);
            assert!(
                u64::from(value)
                    .checked_mul(u64::from(value))
                    .and_then(|n| n.checked_mul(4))
                    .is_some_and(|n| n <= crate::MAX_CAPTURE_BYTES)
            );
        }
    }
    mutation::run("coordinates_and_sizes", &corpus, geometry);
}

#[derive(serde::Deserialize, serde::Serialize)]
struct DragBits {
    start: [u32; 2],
    end: [u32; 2],
    viewport: [u32; 2],
    image: [u32; 2],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expected: Option<PixelRegion>,
}

fn drag(bytes: &[u8]) -> usize {
    let Ok(bits) = serde_json::from_slice::<DragBits>(bytes) else {
        return 0;
    };
    let start = bits.start.map(f32::from_bits);
    let end = bits.end.map(f32::from_bits);
    let viewport = bits.viewport.map(f32::from_bits);
    for square in [false, true] {
        let make = || PixelRegion::from_drag_constrained(start, end, viewport, bits.image, square);
        let first = make();
        assert!(first == make(), "drag mapping changed");
        if !square {
            assert!(first == PixelRegion::from_drag(start, end, viewport, bits.image));
        }
        if let Some(area) = first {
            checked_area(area, bits.image[0], bits.image[1]);
            if square {
                assert!(area.width == area.height);
            }
        }
    }
    std::mem::size_of::<PixelRegion>()
}

#[test]
fn structured_parser_fuzz_float_coordinate_bits() {
    let mut corpus = lines(DRAGS);
    let mut random = mutation::Random(mutation::SEED);
    let edges = [
        0,
        1,
        2,
        3,
        (-0.0_f32).to_bits(),
        f32::MIN_POSITIVE.to_bits(),
        1_f32.to_bits(),
        (-1_f32).to_bits(),
        f32::MAX.to_bits(),
        f32::NAN.to_bits(),
        f32::INFINITY.to_bits(),
        f32::NEG_INFINITY.to_bits(),
    ];
    for _ in 0..128 {
        let mut coordinate = || {
            if random.next().is_multiple_of(2) {
                edges[random.index(edges.len())]
            } else {
                random.next() as u32
            }
        };
        let bits = DragBits {
            start: [coordinate(), coordinate()],
            end: [coordinate(), coordinate()],
            viewport: [coordinate(), coordinate()],
            image: [random.next() as u32, random.next() as u32],
            expected: None,
        };
        corpus.push(serde_json::to_vec(&bits).unwrap());
    }
    mutation::run("float_coordinate_bits", &corpus, drag);
    // Expected fields are trusted only in unmutated fixtures.
    for bytes in lines(DRAGS) {
        let bits: DragBits = serde_json::from_slice(&bytes).unwrap();
        for square in [false, true] {
            assert!(
                PixelRegion::from_drag_constrained(
                    bits.start.map(f32::from_bits),
                    bits.end.map(f32::from_bits),
                    bits.viewport.map(f32::from_bits),
                    bits.image,
                    square,
                ) == bits.expected,
                "fixed drag regression oracle failed"
            );
        }
    }
}

#[test]
fn structured_parser_fuzz_format_paths() {
    for (path, expected) in [
        ("fixture.png", ImageExportFormat::Png),
        ("fixture.JPEG", ImageExportFormat::Jpeg),
        ("🦊.WeBp", ImageExportFormat::WebP),
        ("fixture.jpg", ImageExportFormat::Jpeg),
        ("../fixture.png", ImageExportFormat::Png),
    ] {
        assert!(
            ImageExportFormat::from_path(std::path::Path::new(path)).unwrap() == expected,
            "fixed format answer changed"
        );
    }
    for path in [
        "fixture",
        "fixture.png\0bad",
        "fixture.bmp",
        "fixture.tiff",
        ".png",
        "fixture.png.txt",
    ] {
        assert!(
            ImageExportFormat::from_path(std::path::Path::new(path)).is_err(),
            "unsupported suffix accepted"
        );
    }
    for (width, height, expected) in [
        (1, 1, [true, true, true]),
        (0, 1, [false, false, false]),
        (1, 0, [false, false, false]),
        (8192, 8192, [true, true, true]),
        (8193, 8192, [false, false, false]),
        (16384, 1, [true, true, true]),
        (1, 16384, [true, true, true]),
        (16385, 1, [true, true, false]),
        (1, 16385, [true, true, false]),
        (65535, 1, [true, true, false]),
        (1, 65535, [true, true, false]),
        (65536, 1, [true, false, false]),
        (1, 65536, [true, false, false]),
        (67_108_864, 1, [true, false, false]),
        (67_108_865, 1, [false, false, false]),
        (u32::MAX, 1, [false, false, false]),
        (1, u32::MAX, [false, false, false]),
    ] {
        for (format, accepted) in [
            ImageExportFormat::Png,
            ImageExportFormat::Jpeg,
            ImageExportFormat::WebP,
        ]
        .into_iter()
        .zip(expected)
        {
            assert!(
                format.validate_output_size(width, height).is_ok() == accepted,
                "fixed format axis admission answer changed"
            );
        }
    }
    let corpus = [
        "fixture.png",
        "fixture.JPEG",
        "🦊.WeBp",
        "fixture.jpg",
        "../fixture.png",
        "fixture",
        "fixture.png\0bad",
    ]
    .map(|value| value.as_bytes().to_vec());
    mutation::run("format_paths", &corpus, |bytes| {
        let Ok(text) = std::str::from_utf8(bytes) else {
            return 0;
        };
        let parse = || {
            ImageExportFormat::from_path(std::path::Path::new(text))
                .map_err(|error| error.to_string())
        };
        let first = parse();
        assert!(first == parse(), "format parsing changed");
        if let Ok(format) = first {
            assert!(ImageExportFormat::ALL.contains(&format));
            for (width, height) in [(0, 1), (1, 1), (16384, 1), (65536, 1), (u32::MAX, u32::MAX)] {
                if format.validate_output_size(width, height).is_ok() {
                    assert!(crate::validate_image_size(width, height).is_ok());
                    assert!(format != ImageExportFormat::Jpeg || width <= u32::from(u16::MAX));
                    assert!(format != ImageExportFormat::WebP || width <= 16384);
                }
            }
            format.extension().len()
        } else {
            0
        }
    });
}

fn profile(bytes: &[u8], sharex: bool, file: bool) -> std::result::Result<Vec<u8>, String> {
    let destination = if sharex {
        UploadDestination::from_sharex_snapshot(bytes, file).map(|value| value.0)
    } else {
        UploadDestination::from_json_snapshot(bytes)
    }
    .map_err(|error| error.to_string())?;
    destination.validate().unwrap();
    let normalized = serde_json::to_vec(&destination).unwrap();
    assert!(
        normalized.len() <= 4 * mutation::MAX_INPUT,
        "normalized profile exceeds bounded expansion"
    );
    Ok(normalized)
}

#[test]
fn structured_parser_fuzz_upload_profile_and_template_parsing() {
    for (family, sharex, text) in [
        ("native_upload_profile", false, NATIVE),
        ("sharex_upload_profile", true, SHAREX),
    ] {
        let mut corpus = lines(text);
        for bytes in &corpus {
            assert!(
                profile(bytes, sharex, false).is_ok(),
                "invalid valid-profile seed"
            );
        }
        corpus.push(format!("{}0{}", "[".repeat(160), "]".repeat(160)).into_bytes());
        mutation::run(family, &corpus, |bytes| {
            let mut output = 0;
            for file in [false, true] {
                let first = profile(bytes, sharex, file);
                assert!(
                    first == profile(bytes, sharex, file),
                    "profile parsing changed"
                );
                if let Ok(bytes) = first {
                    output = output.max(bytes.len());
                }
            }
            output
        });
    }
    let seeds = responses();
    let mut corpus: Vec<Vec<u8>> = seeds
        .iter()
        .filter_map(|seed| seed.template.as_ref())
        .map(|value| value.as_bytes().to_vec())
        .collect();
    for value in [
        "{responseurl}",
        "{json:a[0].b}",
        "{regex:\\{([^}]+)\\}|1}",
        "{regex:((((((((((((((((((((((((((((((((((x))))))))))))))))))))))))))))))))))}",
        "{xml:/x[18446744073709551616]/@url}",
        "{xml:/x/../url}",
        "{json:a~1b}",
    ] {
        corpus.push(value.as_bytes().to_vec());
    }
    mutation::run("response_templates", &corpus, |bytes| {
        let Ok(text) = std::str::from_utf8(bytes) else {
            return 0;
        };
        for method in [UploadMethod::Post, UploadMethod::Put] {
            let parse = || sharex_response(text, method).map_err(|error| error.to_string());
            assert!(parse() == parse(), "primary template parsing changed");
        }
        let parse = || {
            UploadResultParser::from_sharex(text, UploadMethod::Post, false, "fixture")
                .map_err(|error| error.to_string())
        };
        let first = parse();
        assert!(first == parse(), "result template parsing changed");
        if let Ok(parser) = first {
            let canonical = parser.to_sharex().unwrap();
            assert!(canonical.len() <= 256);
            assert!(
                UploadResultParser::from_sharex(&canonical, UploadMethod::Post, false, "fixture")
                    .unwrap()
                    == parser
            );
            canonical.len()
        } else {
            0
        }
    });
}

#[derive(serde::Deserialize)]
struct ResponseCase {
    template: Option<String>,
    pointer: Option<String>,
    body: String,
    expected: String,
}
impl ResponseCase {
    fn parser(&self) -> UploadResultParser {
        if let Some(template) = &self.template {
            UploadResultParser::from_sharex(template, UploadMethod::Post, false, "fixture").unwrap()
        } else {
            let parser = UploadResultParser {
                json_pointer: self.pointer.clone().unwrap(),
                response: None,
                link_prefix: String::new(),
                link_suffix: String::new(),
            };
            parser
                .validate(UploadMethod::Post, false, "fixture")
                .unwrap();
            parser
        }
    }
}
fn responses() -> Vec<ResponseCase> {
    RESPONSES
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn extract(parser: &UploadResultParser, bytes: &[u8]) -> std::result::Result<String, String> {
    let text = std::str::from_utf8(bytes).ok();
    let json = serde_json::from_slice::<serde_json::Value>(bytes).ok();
    let mut headers = reqwest::header::HeaderMap::new();
    if let Ok(value) = reqwest::header::HeaderValue::from_bytes(bytes) {
        headers.insert("Location", value);
    }
    parser
        .extract(&headers, json.as_ref(), text, "fixture")
        .map_err(|error| error.to_string())
}

fn checked_link(link: &str) {
    // The extractor checks 2048 input bytes; URL canonicalization can percent-encode UTF-8.
    assert!(
        link.len() <= 3 * 2048 + 256,
        "extracted link exceeds bounded canonical expansion"
    );
    let url = reqwest::Url::parse(link).unwrap();
    assert!(matches!(url.scheme(), "http" | "https") && url.host_str().is_some());
    assert!(url.username().is_empty() && url.password().is_none());
    let outcome = UploadOutcome {
        url: link.into(),
        thumbnail: None,
        deletion: None,
    };
    if let Ok(accepted) = outcome.validate_links() {
        assert!(accepted.url.len() <= 2048);
    }
}

#[test]
fn structured_parser_fuzz_uploader_response_extraction() {
    let cases = responses();
    let parsers: Vec<_> = cases.iter().map(ResponseCase::parser).collect();
    for (case, parser) in cases.iter().zip(&parsers) {
        assert!(
            extract(parser, case.body.as_bytes()).unwrap() == case.expected,
            "response seed oracle failed"
        );
    }
    let mut corpus: Vec<_> = cases
        .iter()
        .map(|case| case.body.as_bytes().to_vec())
        .collect();
    for body in [
        "javascript:alert(1)",
        "https://user:password@example.invalid/",
        "file:///fixture-only",
        "//example.invalid/x",
        "https://example.invalid/🦊",
        "https://example.invalid/\u{2028}",
        "<!DOCTYPE x [<!ENTITY e SYSTEM 'file:///fixture-only'>]><x>&e;</x>",
        "<files><file><url>",
        "{\"result\":{\"url\":1e309}}",
    ] {
        corpus.push(body.as_bytes().to_vec());
    }
    corpus.push(format!("{}0{}", "[".repeat(160), "]".repeat(160)).into_bytes());
    mutation::run("uploader_responses", &corpus, |bytes| {
        let mut output = 0;
        for parser in &parsers {
            let first = extract(parser, bytes);
            assert!(
                first == extract(parser, bytes),
                "response extraction changed"
            );
            if let Ok(link) = first {
                checked_link(&link);
                output = output.max(link.len());
            }
        }
        output
    });
}

#[test]
fn structured_parser_fuzz_boundary_and_private_errors() {
    const PRIVATE: &str = "synthetic-private-marker-🦊-2714";
    let started = std::time::Instant::now();
    let mut max_output = 0;
    let cases = responses();
    let parsers: Vec<_> = cases.iter().map(ResponseCase::parser).collect();
    // Streaming admission owns MAX_RESPONSE; only its admitted byte count enters pure parsers.
    let at_limit = vec![b'x'; MAX_RESPONSE];
    for parser in &parsers {
        assert!(extract(parser, &at_limit).is_err());
    }
    let too_many_nodes = format!(
        "<files>{}</files>",
        "<file/>".repeat(MAX_XML_NODES as usize)
    );
    assert!(xml_capture(&too_many_nodes, "/files/file", "fixture").is_err());
    for depth in [8, 32, 128, 160] {
        let body = format!("{}x{}", "<x>".repeat(depth), "</x>".repeat(depth));
        let first = xml_capture(&body, "/x", "fixture").map_err(|error| error.to_string());
        assert!(first == xml_capture(&body, "/x", "fixture").map_err(|error| error.to_string()));
        if let Ok(value) = first {
            max_output = max_output.max(value.len());
        }
    }
    let header_parser =
        UploadResultParser::from_sharex("{header:Location}", UploadMethod::Post, false, "fixture")
            .unwrap();
    let mut headers = reqwest::header::HeaderMap::new();
    for _ in 0..2 {
        headers.append(
            "Location",
            reqwest::header::HeaderValue::from_static("https://example.invalid/"),
        );
    }
    assert!(
        header_parser
            .extract(&headers, None, None, "fixture")
            .is_err()
    );
    for value in [
        serde_json::json!(PRIVATE),
        serde_json::json!({(PRIVATE):true}),
        serde_json::json!([PRIVATE]),
        serde_json::json!(false),
    ] {
        let profile = serde_json::json!({"DestinationType":"ImageUploader, FileUploader", "RequestURL":"https://example.invalid/upload", "URL":"{response}", "ErrorMessage":value});
        for file in [false, true] {
            let result = UploadDestination::from_sharex_snapshot(
                &serde_json::to_vec(&profile).unwrap(),
                file,
            );
            let error = result.err().expect("ShareX ErrorMessage must be rejected");
            assert!(error.to_string() == "ShareX ErrorMessage parsing is not supported");
            assert!(
                !format!("{error:#}").contains(PRIVATE),
                "protected error echoed private value"
            );
        }
    }
    let error = UploadOutcome {
        url: format!("https://{PRIVATE}:password@example.invalid/"),
        thumbnail: None,
        deletion: None,
    }
    .validate_links()
    .unwrap_err();
    assert!(
        !format!("{error:#}").contains(PRIVATE),
        "protected link error echoed private value"
    );
    eprintln!(
        "STRUCTURED_FUZZ {{\"family\":\"boundary_and_private_errors\",\"seed\":\"{:#x}\",\"cases\":24,\"max_input\":{},\"max_output\":{max_output},\"xml_node_boundary\":{},\"xml_max_depth\":160,\"elapsed_us\":{}}}",
        mutation::SEED,
        MAX_RESPONSE,
        MAX_XML_NODES + 1,
        started.elapsed().as_micros()
    );
}
