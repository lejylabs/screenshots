//! Complete bounded native inventory; empty slices query the current total.
pub(crate) const MAX_ACTIVE_DISPLAYS: usize = 32;

pub(crate) fn complete_active_display_ids(
    mut query: impl FnMut(&mut [u32]) -> Result<usize, ()>,
) -> Result<Vec<u32>, &'static str> {
    let expected = query(&mut []).map_err(|_| "Cannot query native display count")?;
    if !(1..=MAX_ACTIVE_DISPLAYS).contains(&expected) {
        return Err("Native display inventory must contain 1–32 displays");
    }
    let mut ids = [0; MAX_ACTIVE_DISPLAYS];
    let returned = query(&mut ids).map_err(|_| "Cannot enumerate native displays")?;
    let confirmed = query(&mut []).map_err(|_| "Cannot confirm native display count")?;
    if returned != expected || confirmed != expected {
        return Err("Native display inventory changed or was truncated");
    }
    let ids = &ids[..expected];
    if ids
        .iter()
        .enumerate()
        .any(|(at, id)| *id == 0 || ids[..at].contains(id))
    {
        return Err("Native display inventory contains invalid or duplicate identities");
    }
    Ok(ids.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complete_inventory_accepts_all_seventeen_and_thirty_two_identities() {
        for count in [1, 16, 17, 32] {
            let mut calls = 0;
            let ids = complete_active_display_ids(|output| {
                calls += 1;
                if !output.is_empty() {
                    assert_eq!(output.len(), 32);
                    for (at, id) in output[..count].iter_mut().enumerate() {
                        *id = at as u32 + 1;
                    }
                }
                Ok(count)
            })
            .unwrap();
            assert_eq!(ids, (1..=count as u32).collect::<Vec<_>>());
            assert_eq!(calls, 3);
        }
    }

    #[test]
    fn oversized_count_is_rejected_before_the_list_buffer_is_requested() {
        let mut calls = 0;
        assert!(
            complete_active_display_ids(|output| {
                calls += 1;
                assert!(output.is_empty());
                Ok(33)
            })
            .is_err()
        );
        assert_eq!(calls, 1);
    }

    #[test]
    fn partial_or_changed_inventories_never_return_a_successful_subset() {
        for counts in [[17, 16, 17], [17, 17, 18], [32, 32, 33], [16, 17, 16]] {
            let mut at = 0;
            assert!(
                complete_active_display_ids(|output| {
                    let count = counts[at];
                    at += 1;
                    for (at, id) in output.iter_mut().take(count).enumerate() {
                        *id = at as u32 + 1;
                    }
                    Ok(count)
                })
                .is_err()
            );
        }
        for duplicate in [false, true] {
            assert!(
                complete_active_display_ids(|output| {
                    if !output.is_empty() {
                        output[0] = 1;
                        output[1] = u32::from(duplicate);
                    }
                    Ok(2)
                })
                .is_err()
            );
        }
    }
}
