//! Native X11 raster bounds, independent of resource-manager/UI scale.

pub(crate) fn physical_region_origin(
    geometry: (i32, i32, u32, u32),
    area: (u32, u32, u32, u32),
) -> Option<(i32, i32)> {
    let (left, top, monitor_width, monitor_height) = geometry;
    let (x, y, width, height) = area;
    if width == 0
        || height == 0
        || x.checked_add(width)? > monitor_width
        || y.checked_add(height)? > monitor_height
        || u16::try_from(width).is_err()
        || u16::try_from(height).is_err()
    {
        return None;
    }
    let x = i64::from(left) + i64::from(x);
    let y = i64::from(top) + i64::from(y);
    Some((
        i32::from(i16::try_from(x).ok()?),
        i32::from(i16::try_from(y).ok()?),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_raster_admits_far_half_and_refuses_adjacent_monitor() {
        let first = (0, 0, 1920, 1080);
        assert_eq!(
            physical_region_origin(first, (1500, 500, 1, 1)),
            Some((1500, 500))
        );
        assert_eq!(
            physical_region_origin(first, (1919, 1079, 1, 1)),
            Some((1919, 1079))
        );
        assert_eq!(physical_region_origin(first, (2500, 500, 1, 1)), None);
        assert_eq!(physical_region_origin(first, (1920, 0, 1, 1)), None);
        assert_eq!(physical_region_origin(first, (0, 1080, 1, 1)), None);
        let second = (1920, 0, 1920, 1080);
        assert_eq!(
            physical_region_origin(second, (580, 500, 1, 1)),
            Some((2500, 500))
        );
    }

    #[test]
    fn negative_origins_and_overflow_are_checked_before_native_narrowing() {
        assert_eq!(
            physical_region_origin((-1920, -1080, 1920, 1080), (10, 20, 1, 1)),
            Some((-1910, -1060))
        );
        assert_eq!(
            physical_region_origin((0, 0, u32::MAX, u32::MAX), (u32::MAX, 0, 1, 1)),
            None
        );
        assert_eq!(
            physical_region_origin((i32::MAX, 0, 2, 2), (1, 0, 1, 1)),
            None
        );
        assert_eq!(
            physical_region_origin((i32::MIN, 0, 2, 2), (0, 0, 1, 1)),
            None
        );
        assert_eq!(physical_region_origin((32767, 0, 2, 2), (1, 0, 1, 1)), None);
        assert_eq!(physical_region_origin((0, 0, 2, 2), (0, 0, 0, 1)), None);
        assert_eq!(
            physical_region_origin((0, 0, u32::MAX, 1), (0, 0, 65536, 1)),
            None
        );
    }
}
