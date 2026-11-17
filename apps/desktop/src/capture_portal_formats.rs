//! Source-proven managed alternatives followed by the unchanged RGB8 assumption.
use app_core::{CaptureError, PortalStreamSource};
use pipewire::spa;
use spa::{
    param::{format::FormatProperties as Key, video::VideoFormat as Video},
    pod::{Object, Property, PropertyFlags, Value},
};

fn choices(key: Key, values: &[u32], required: bool) -> Property {
    Property {
        key: key.as_raw(),
        flags: if required {
            PropertyFlags::MANDATORY
        } else {
            PropertyFlags::empty()
        },
        value: Value::Choice(spa::pod::ChoiceValue::Id(spa::utils::Choice(
            spa::utils::ChoiceFlags::empty(),
            spa::utils::ChoiceEnum::Enum {
                default: spa::utils::Id(values[0]),
                alternatives: values.iter().copied().map(spa::utils::Id).collect(),
            },
        ))),
    }
}
fn fixed(key: Key, value: u32) -> Property {
    Property {
        key: key.as_raw(),
        flags: PropertyFlags::MANDATORY,
        value: Value::Id(spa::utils::Id(value)),
    }
}
fn base(source: Option<PortalStreamSource>) -> Object {
    spa::pod::object!(
        spa::utils::SpaTypes::ObjectParamFormat,
        spa::param::ParamType::EnumFormat,
        spa::pod::property!(Key::MediaType, Id, spa::param::format::MediaType::Video),
        spa::pod::property!(Key::MediaSubtype, Id, spa::param::format::MediaSubtype::Raw),
        spa::pod::property!(
            Key::VideoSize,
            Choice,
            Range,
            Rectangle,
            spa::utils::Rectangle {
                width: source.map_or(1920, |s| s.width),
                height: source.map_or(1080, |s| s.height)
            },
            spa::utils::Rectangle {
                width: source.map_or(1, |s| s.width),
                height: source.map_or(1, |s| s.height)
            },
            spa::utils::Rectangle {
                width: source.map_or(16384, |s| s.width),
                height: source.map_or(16384, |s| s.height)
            }
        ),
        spa::pod::property!(
            Key::VideoFramerate,
            Choice,
            Range,
            Fraction,
            spa::utils::Fraction { num: 30, denom: 1 },
            spa::utils::Fraction { num: 0, denom: 1 },
            spa::utils::Fraction { num: 240, denom: 1 }
        )
    )
}
fn serialize(object: Object) -> Result<Vec<u8>, CaptureError> {
    spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &Value::Object(object),
    )
    .map(|value| value.0.into_inner())
    .map_err(|_| CaptureError::Backend("Portal format negotiation failed".into()))
}
/// MANDATORY means both sides must supply the property when SPA filters pods.
/// It prevents the consumer's defaults from inventing absent source color tags.
pub(crate) fn offers(source: Option<PortalStreamSource>) -> Result<Vec<Vec<u8>>, CaptureError> {
    use spa::sys::*;
    let rgb8 = [
        Video::BGRx.as_raw(),
        Video::RGBx.as_raw(),
        Video::BGRA.as_raw(),
        Video::RGBA.as_raw(),
    ];
    let mut precision = Vec::with_capacity(10);
    if cfg!(target_endian = "little") {
        precision.extend([Video::RGBA_F16.as_raw(), Video::RGBA_F32.as_raw()]);
    }
    precision.extend([
        Video::xBGR_210LE.as_raw(),
        Video::xRGB_210LE.as_raw(),
        Video::RGBx_102LE.as_raw(),
        Video::BGRx_102LE.as_raw(),
        Video::ARGB_210LE.as_raw(),
        Video::ABGR_210LE.as_raw(),
        Video::RGBA_102LE.as_raw(),
        Video::BGRA_102LE.as_raw(),
    ]);
    let sdr = [
        SPA_VIDEO_TRANSFER_GAMMA10,
        SPA_VIDEO_TRANSFER_SRGB,
        SPA_VIDEO_TRANSFER_BT709,
        SPA_VIDEO_TRANSFER_BT601,
        SPA_VIDEO_TRANSFER_BT2020_10,
        SPA_VIDEO_TRANSFER_BT2020_12,
        SPA_VIDEO_TRANSFER_GAMMA22,
        SPA_VIDEO_TRANSFER_ADOBERGB,
    ];
    let mut hdr = sdr.to_vec();
    hdr.extend([
        SPA_VIDEO_TRANSFER_SMPTE2084,
        SPA_VIDEO_TRANSFER_ARIB_STD_B67,
    ]);
    let primaries = [
        SPA_VIDEO_COLOR_PRIMARIES_BT709,
        SPA_VIDEO_COLOR_PRIMARIES_SMPTEEG432,
        SPA_VIDEO_COLOR_PRIMARIES_BT2020,
        SPA_VIDEO_COLOR_PRIMARIES_ADOBERGB,
    ];
    let mut result = Vec::with_capacity(3);
    for (formats, transfers) in [
        (precision.as_slice(), hdr.as_slice()),
        (rgb8.as_slice(), sdr.as_slice()),
    ] {
        let mut object = base(source);
        object.properties.extend([
            choices(Key::VideoFormat, formats, false),
            fixed(Key::VideoColorRange, SPA_VIDEO_COLOR_RANGE_0_255),
            fixed(Key::VideoColorMatrix, SPA_VIDEO_COLOR_MATRIX_RGB),
            choices(Key::VideoColorPrimaries, &primaries, true),
            choices(Key::VideoTransferFunction, transfers, true),
        ]);
        result.push(serialize(object)?);
    }
    // Optional legacy defaults are UNKNOWN, never fabricated full/709/sRGB tags.
    // Restrict real provided tags to the exact previously accepted RGB8 policy.
    let mut legacy = base(source);
    legacy.properties.extend([
        choices(Key::VideoFormat, &rgb8, false),
        choices(
            Key::VideoColorRange,
            &[0, SPA_VIDEO_COLOR_RANGE_0_255],
            false,
        ),
        choices(
            Key::VideoColorMatrix,
            &[0, SPA_VIDEO_COLOR_MATRIX_RGB],
            false,
        ),
        choices(
            Key::VideoColorPrimaries,
            &[0, SPA_VIDEO_COLOR_PRIMARIES_BT709],
            false,
        ),
        choices(
            Key::VideoTransferFunction,
            &[0, SPA_VIDEO_TRANSFER_SRGB],
            false,
        ),
    ]);
    result.push(serialize(legacy)?);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn managed_offers_require_each_source_color_property_and_legacy_does_not() {
        let offers = offers(None).unwrap();
        assert_eq!(offers.len(), 3);
        assert!(offers.iter().all(|offer| offer.len() <= 1024));
        for (index, bytes) in offers.iter().enumerate() {
            let pod = spa::pod::Pod::from_bytes(bytes)
                .unwrap()
                .as_object()
                .unwrap();
            for key in [
                Key::VideoColorRange,
                Key::VideoColorMatrix,
                Key::VideoColorPrimaries,
                Key::VideoTransferFunction,
            ] {
                let property = pod.find_prop(spa::utils::Id(key.as_raw())).unwrap();
                assert_eq!(
                    property.flags().contains(spa::pod::PodPropFlags::MANDATORY),
                    index < 2
                );
            }
        }
    }
}
