#![cfg(target_os = "linux")]

use glib::{Variant, variant::ToVariant};

#[test]
fn borrowed_variant_strings_survive_forward_and_reverse_iteration() {
    let values = ["first", "Grüße", "", "最後"];
    let variant = Variant::array_from_iter::<String>(values.map(ToVariant::to_variant));
    assert_eq!(
        variant.array_iter_str().unwrap().collect::<Vec<_>>(),
        values
    );
    let mut iter = variant.array_iter_str().unwrap();
    assert_eq!(iter.next_back(), Some("最後"));
    assert_eq!(iter.next(), Some("first"));
    assert_eq!(iter.nth(1), Some(""));
    assert_eq!(iter.next_back(), None);
    assert_eq!(iter.next(), None);
}
