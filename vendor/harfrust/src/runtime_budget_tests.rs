//! Actual private-engine boundaries for the editor's opt-in runtime refusal.
use crate::{Buffer, Direction, Font, Script, ShapePlan, ShaperFont};
#[path = "../../../crates/app-core/tests/support/editor_kern_fonts.rs"]
mod fixtures;

fn state_body(flags: u16) -> Vec<u8> {
    let mut body = fixtures::kern_words([4, 8, 128, 136, 0, 116]);
    let mut classes = [1; 116];
    let mut states = [0; 8];
    if flags == 0 {
        // A starts an actionable transition, admitting this table through the
        // real initial-glyph cache; V then enters the nonactionable fast path.
        classes[4] = 2;
        states[2] = 1;
        states[6] = 1;
    }
    body.extend(classes);
    body.extend(states);
    body.extend(fixtures::kern_words([128, flags, 128, 0x8000]));
    body
}

fn buffer() -> Buffer {
    let mut buffer = Buffer::new();
    buffer.push_str("AV");
    buffer.set_direction(Direction::LeftToRight);
    buffer.set_script(Some(Script::LATIN));
    for (info, glyph) in buffer.info.iter_mut().zip([4, 115]) {
        info.glyph_id = glyph;
        info.mask = !0;
    }
    buffer.clear_positions();
    buffer
}

fn run_kern(remaining: i32, flags: u16, tables: u32, strict: bool) -> Buffer {
    let bytes =
        fixtures::font_with_legacy_kern(&fixtures::apple_kern(1, &state_body(flags), tables));
    let font = Font::new(bytes, 0).unwrap();
    let shaper = ShaperFont::new(&font);
    let layout = shaper.layout();
    let plan = ShapePlan::from_layout(
        layout,
        Direction::LeftToRight,
        Some(Script::LATIN),
        None,
        &[],
    );
    let mut buffer = buffer();
    buffer.max_ops = remaining;
    buffer.set_fail_on_runtime_budget_exhaustion(strict);
    crate::aat::kern::apply(
        &plan,
        layout,
        shaper.scale(),
        &mut buffer,
        #[cfg(feature = "tracing")]
        &shaper,
    )
    .unwrap();
    buffer
}

#[test]
fn runtime_kern_fast_and_main_paths_accept_natural_exact_zero_and_refuse_extra_work() {
    for flags in [0, 0x8000] {
        let exact = run_kern(2, flags, 1, true);
        assert!(exact.allocation_successful());
        assert_eq!((exact.max_ops, exact.idx), (0, 2));
        let exhausted = run_kern(1, flags, 1, true);
        assert!(!exhausted.allocation_successful());
        assert_eq!((exhausted.max_ops, exhausted.idx), (-1, 1));
        let original = run_kern(1, flags, 1, false);
        assert!(original.allocation_successful());
        assert_eq!((original.max_ops, original.idx), (-1, 2));
    }
}

#[test]
fn runtime_kern_forced_zero_refuses_and_stops_later_subtables() {
    let strict = run_kern(1, 0xc000, 2, true);
    assert!(!strict.allocation_successful());
    assert_eq!((strict.max_ops, strict.idx), (0, 0));
    let original = run_kern(1, 0xc000, 2, false);
    assert!(original.allocation_successful());
    assert_eq!(original.idx, 2);
}

#[test]
fn runtime_ot_zero_only_refuses_when_a_required_recursion_is_skipped() {
    use crate::ot::{
        apply::{apply_lookup, ApplyContext},
        layout::LayoutTableKind,
    };
    use read_fonts::tables::layout::SequenceLookupRecord;
    let bytes = include_bytes!("../../cosmic-text/fonts/FiraMono-Medium.ttf").to_vec();
    let font = Font::new(bytes, 0).unwrap();
    let shaper = ShaperFont::new(&font);
    let lookup = SequenceLookupRecord {
        sequence_index: 0.into(),
        lookup_list_index: 0.into(),
    };
    for (strict, required) in [(true, false), (true, true), (false, true)] {
        let mut buffer = buffer();
        buffer.max_ops = 0;
        buffer.set_fail_on_runtime_budget_exhaustion(strict);
        let mut ctx = ApplyContext::new(
            LayoutTableKind::Gpos,
            shaper.layout(),
            shaper.scale(),
            &mut buffer,
            #[cfg(feature = "tracing")]
            &shaper,
        );
        ctx.match_positions[0] = 0;
        apply_lookup(
            &mut ctx,
            0,
            0,
            if required {
                core::slice::from_ref(&lookup)
            } else {
                &[]
            },
        );
        assert_eq!(buffer.allocation_successful(), !(strict && required));
    }
}

#[test]
fn runtime_budget_failure_survives_leave_clear_retains_option_and_reset_disables_it() {
    let mut buffer = Buffer::new();
    buffer.set_fail_on_runtime_budget_exhaustion(true);
    assert!(buffer.fail_runtime_budget());
    buffer.leave();
    assert!(!buffer.allocation_successful());
    buffer.clear();
    assert!(buffer.allocation_successful());
    assert!(buffer.fail_runtime_budget());
    assert!(!buffer.allocation_successful());
    buffer.reset();
    assert!(!buffer.fail_runtime_budget());
    assert!(buffer.allocation_successful());
}
