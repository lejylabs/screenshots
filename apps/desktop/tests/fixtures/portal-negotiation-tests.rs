// Included by the owned private portal fixture; these are metadata-only SDK checks.
fn native_filter_binary(root: &Path) -> Result<PathBuf> {
    let source = root.join("spa-filter.c");
    std::fs::write(
        &source,
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/spa-pod-filter.c"
        )),
    )?;
    let mut pkg = Command::new("pkg-config");
    pkg.args(["--cflags", "libspa-0.2"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    parent_death(&mut pkg);
    let flags = run(&mut pkg, Duration::from_secs(3))?;
    ensure!(
        flags.status.success() && flags.stdout.len() <= 4096,
        "Private SPA headers unavailable"
    );
    let output = root.join("spa-filter");
    let mut cc = Command::new("cc");
    cc.args(["-std=gnu11", "-O2", "-Wall", "-Wextra"])
        .arg(&source)
        .arg("-o")
        .arg(&output)
        .args(std::str::from_utf8(&flags.stdout)?.split_whitespace())
        .arg("-lm")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    parent_death(&mut cc);
    let result = run(&mut cc, Duration::from_secs(15))?;
    ensure!(
        result.status.success(),
        "Private native SPA oracle compile failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    Ok(output)
}
fn source_format_pod(
    video: pipewire::spa::param::video::VideoFormat,
    color: Option<[u32; 4]>,
) -> Result<Vec<u8>> {
    source_format_pod_fields(video, color, None)
}
fn source_format_pod_fields(
    video: pipewire::spa::param::video::VideoFormat,
    color: Option<[u32; 4]>,
    omit: Option<u32>,
) -> Result<Vec<u8>> {
    use pipewire::spa;
    let mut object = spa::pod::object!(
        spa::utils::SpaTypes::ObjectParamFormat,
        spa::param::ParamType::EnumFormat,
        spa::pod::property!(
            spa::param::format::FormatProperties::MediaType,
            Id,
            spa::param::format::MediaType::Video
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::MediaSubtype,
            Id,
            spa::param::format::MediaSubtype::Raw
        ),
        spa::pod::property!(spa::param::format::FormatProperties::VideoFormat, Id, video),
        spa::pod::property!(
            spa::param::format::FormatProperties::VideoSize,
            Rectangle,
            spa::utils::Rectangle {
                width: 4,
                height: 3
            }
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::VideoFramerate,
            Fraction,
            spa::utils::Fraction { num: 30, denom: 1 }
        )
    );
    if let Some(values) = color {
        for (key, value) in [
            spa::sys::SPA_FORMAT_VIDEO_colorRange,
            spa::sys::SPA_FORMAT_VIDEO_colorMatrix,
            spa::sys::SPA_FORMAT_VIDEO_colorPrimaries,
            spa::sys::SPA_FORMAT_VIDEO_transferFunction,
        ]
        .into_iter()
        .zip(values)
        {
            if omit != Some(key) {
                object.properties.push(spa::pod::Property::new(
                    key,
                    spa::pod::Value::Id(spa::utils::Id(value)),
                ));
            }
        }
    }
    Ok(spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &spa::pod::Value::Object(object),
    )?
    .0
    .into_inner())
}
fn dual_source_format_pod() -> Result<Vec<u8>> {
    use pipewire::spa;
    let object = spa::pod::object!(
        spa::utils::SpaTypes::ObjectParamFormat,
        spa::param::ParamType::EnumFormat,
        spa::pod::property!(
            spa::param::format::FormatProperties::MediaType,
            Id,
            spa::param::format::MediaType::Video
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::MediaSubtype,
            Id,
            spa::param::format::MediaSubtype::Raw
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::VideoFormat,
            Choice,
            Enum,
            Id,
            spa::param::video::VideoFormat::RGBA_F16,
            spa::param::video::VideoFormat::RGBA_F16,
            spa::param::video::VideoFormat::RGBA
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::VideoSize,
            Rectangle,
            spa::utils::Rectangle {
                width: 4,
                height: 3
            }
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::VideoFramerate,
            Fraction,
            spa::utils::Fraction { num: 30, denom: 1 }
        )
    );
    Ok(spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &spa::pod::Value::Object(object),
    )?
    .0
    .into_inner())
}
#[test]
#[ignore = "explicit native installed-SPA header oracle; generated metadata only"]
fn managed_format_offers_use_real_spa_mandatory_filtering() -> Result<()> {
    use pipewire::spa;
    let root = Folder::new();
    let binary = native_filter_binary(&root.0)?;
    let offers = negotiated_formats::offers(None)?;
    let full = [
        spa::sys::SPA_VIDEO_COLOR_RANGE_0_255,
        spa::sys::SPA_VIDEO_COLOR_MATRIX_RGB,
        spa::sys::SPA_VIDEO_COLOR_PRIMARIES_BT2020,
        spa::sys::SPA_VIDEO_TRANSFER_GAMMA10,
    ];
    for reverse in [false, true] {
        for (name, video, color, expected) in [
            (
                "untagged-float",
                spa::param::video::VideoFormat::RGBA_F16,
                None,
                vec![false, false, false],
            ),
            (
                "untagged-rgb8",
                spa::param::video::VideoFormat::RGBA,
                None,
                vec![false, false, true],
            ),
            (
                "tagged-float",
                spa::param::video::VideoFormat::RGBA_F16,
                Some(full),
                vec![cfg!(target_endian = "little"), false, false],
            ),
            (
                "tagged-rgb8",
                spa::param::video::VideoFormat::RGBA,
                Some(full),
                vec![false, true, false],
            ),
        ] {
            let source = root.0.join("source.pod");
            std::fs::write(&source, source_format_pod(video, color)?)?;
            for (index, offer) in offers.iter().enumerate() {
                let file = root.0.join("offer.pod");
                std::fs::write(&file, offer)?;
                let mut command = Command::new(&binary);
                command
                    .arg(&source)
                    .arg(&file)
                    .arg(if reverse { "reverse" } else { "forward" })
                    .stdin(Stdio::null())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped());
                parent_death(&mut command);
                let result = run(&mut command, Duration::from_secs(3))?;
                ensure!(
                    result.status.success() == expected[index]
                        && (result.status.success() || result.status.code() == Some(10)),
                    "Native SPA filter mismatch: {name}, offer {index}, reverse={reverse}"
                );
                if result.status.success() {
                    let mut info = spa::param::video::VideoInfoRaw::new();
                    info.parse(
                        spa::pod::Pod::from_bytes(&result.stdout)
                            .context("Native filter returned invalid pod")?,
                    )?;
                    ensure!(
                        info.format() == video
                            && info.size()
                                == spa::utils::Rectangle {
                                    width: 4,
                                    height: 3
                                },
                        "Native filtering changed the source raster/format"
                    );
                    ensure!(
                        [
                            info.color_range(),
                            info.color_matrix(),
                            info.color_primaries(),
                            info.transfer_function()
                        ] == color.unwrap_or([0; 4]),
                        "Native filtering invented or changed color tags"
                    );
                }
            }
        }
    }
    let source = root.0.join("dual-source.pod");
    std::fs::write(&source, dual_source_format_pod()?)?;
    for reverse in [false, true] {
        for (index, offer) in offers.iter().enumerate() {
            let file = root.0.join("offer.pod");
            std::fs::write(&file, offer)?;
            let mut command = Command::new(&binary);
            command
                .arg(&source)
                .arg(&file)
                .arg(if reverse { "reverse" } else { "forward" })
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            parent_death(&mut command);
            let result = run(&mut command, Duration::from_secs(3))?;
            ensure!(
                result.status.success() == (index == 2)
                    && (result.status.success() || result.status.code() == Some(10)),
                "Dual source did not filter exclusively to legacy RGB8"
            );
            if result.status.success() {
                let mut info = spa::param::video::VideoInfoRaw::new();
                info.parse(
                    spa::pod::Pod::from_bytes(&result.stdout)
                        .context("Invalid filtered dual pod")?,
                )?;
                ensure!(
                    info.format() == spa::param::video::VideoFormat::RGBA
                        && [
                            info.color_range(),
                            info.color_matrix(),
                            info.color_primaries(),
                            info.transfer_function()
                        ] == [0; 4],
                    "Dual-source filter selected float or invented color tags"
                );
            }
        }
    }
    for omit in [
        spa::sys::SPA_FORMAT_VIDEO_colorRange,
        spa::sys::SPA_FORMAT_VIDEO_colorMatrix,
        spa::sys::SPA_FORMAT_VIDEO_colorPrimaries,
        spa::sys::SPA_FORMAT_VIDEO_transferFunction,
    ] {
        let source = root.0.join("missing-one.pod");
        std::fs::write(
            &source,
            source_format_pod_fields(
                spa::param::video::VideoFormat::RGBA_F16,
                Some(full),
                Some(omit),
            )?,
        )?;
        for reverse in [false, true] {
            for offer in &offers {
                let file = root.0.join("offer.pod");
                std::fs::write(&file, offer)?;
                let mut command = Command::new(&binary);
                command
                    .arg(&source)
                    .arg(&file)
                    .arg(if reverse { "reverse" } else { "forward" })
                    .stdin(Stdio::null())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped());
                parent_death(&mut command);
                let result = run(&mut command, Duration::from_secs(3))?;
                ensure!(
                    result.status.code() == Some(10),
                    "Native filtering accepted a missing mandatory source property {omit}"
                );
            }
        }
    }
    Ok(())
}
