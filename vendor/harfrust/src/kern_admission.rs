//! Allocation-free bounds for the existing legacy `kern` cache constructor.
//!
//! This observer is opt-in; it does not change upstream cache/shaping behavior.
use crate::aat::{common::ClassCache, kern::KernSubtableCache, KernTableCache};
use crate::budget::Budget;
use alloc::sync::Arc;
use core::mem::size_of;
use read_fonts::{
    model::Tables,
    tables::{aat, kern::SubtableKind},
    FontRef, TableProvider,
};
use yoke::Yoke;

const LIMIT: usize = 4 << 20;
const PAGE_BITS: usize = 512;
// read-fonts 0.46's BitPage ([u64; 8], u32 length) and PageInfo (two u32s).
const PAGE_BYTES: usize = size_of::<([u64; 8], u32)>() + size_of::<(u32, u32)>();

// These vectors start empty and grow by single push/insert. Rust 1.100's
// RawVec::grow_amortized doubles capacity from a minimum of four elements.
fn pushed_capacity(count: usize) -> Option<usize> {
    if count == 0 {
        Some(0)
    } else {
        count.checked_next_power_of_two().map(|n| n.max(4))
    }
}
fn set_bytes(pages: usize) -> Option<usize> {
    pushed_capacity(pages)?.checked_mul(PAGE_BYTES)
}
// SafeToBreak uses reserve/resize by chunks; new capacity is max(2*old, needed).
// Twice final length (or the initial four slots) bounds any such capacity.
fn bulk_bytes<T>(count: usize) -> Option<usize> {
    if count == 0 {
        Some(0)
    } else {
        count.max(4).checked_mul(2)?.checked_mul(size_of::<T>())
    }
}
fn range_pages(first: usize, count: usize) -> Option<usize> {
    // The existing format2/3 collector inserts its first glyph even at count0.
    Some(first.checked_add(count.saturating_sub(1))? / PAGE_BITS - first / PAGE_BITS + 1)
}
#[derive(Default)]
struct Cost {
    entries: usize,
    boxes: usize,
    sets: usize,
    classes: usize,
    tails: usize,
    temporary: usize,
}
impl Cost {
    fn peak(&self) -> Option<usize> {
        let entries =
            pushed_capacity(self.entries)?.checked_mul(size_of::<KernSubtableCache<'_>>())?;
        let retained = entries
            .checked_add(self.boxes)?
            .checked_add(self.sets)?
            .checked_add(bulk_bytes::<u16>(self.classes)?)?
            .checked_add(bulk_bytes::<u64>(self.tails)?)?;
        // Retained + a whole old allocation during growth + temporary class set.
        // Doubling every retained allocation is conservative even though only
        // one vector can reallocate at a time. Inline fields live in the yoke.
        retained
            .checked_add(self.temporary)?
            .checked_mul(2)?
            .checked_add(size_of::<Yoke<KernTableCache<'static>, Arc<Tables>>>())
    }
}

/// Returns a conservative peak-byte allowance for legacy `kern` preprocessing.
///
/// Parses with the same read-fonts types as the cache, without allocating or
/// creating a ShaperFont/cache. `None` means malformed data, arithmetic overflow,
/// or more than 4 MiB peak heap / 4 Mi record-and-state-cell visits. A missing
/// `kern` table returns zero. All legacy formats 0, 1, 2 and 3 are supported.
/// Charge the result separately from the OpenType/text allowance before shaping.
/// The byte proof depends on the locked read-fonts 0.46 sparse-set layout and
/// Rust 1.100 vector growth; see LOCAL-PATCH.md before changing either.
pub fn kern_preprocessing_size(data: &[u8], index: u32) -> Option<usize> {
    let font = FontRef::from_index(data, index).ok()?;
    if font
        .data_for_tag(read_fonts::types::Tag::new(b"kern"))
        .is_none()
    {
        return Some(0);
    }
    let kern = font.kern().ok()?;
    let budget = Budget::new(LIMIT, LIMIT);
    let mut cost = Cost::default();
    let mut peak = cost.peak()?;
    budget.alloc::<u8>(peak).ok()?;
    let expected = match &kern {
        read_fonts::tables::kern::Kern::Ot(table) => usize::from(table.n_tables()),
        read_fonts::tables::kern::Kern::Aat(table) => usize::try_from(table.n_tables()).ok()?,
    };
    // The shared iterator stops on zero length or exhausted physical data.
    // Unlike cache construction, malformed headers/unknown kinds are refused.
    for subtable in kern.subtables() {
        budget.work(1).ok()?;
        let subtable = subtable.ok()?;
        let (first, second, visits) = match subtable.kind().ok()? {
            SubtableKind::Format0(table) => {
                let count = table.pairs().len();
                // GlyphId16 pairs can occupy at most the first 128 pages.
                (count.min(128), count.min(128), count.checked_mul(2)?)
            }
            SubtableKind::Format1(machine) => {
                let classes = usize::from(machine.header.state_size());
                let wouldbe = classes
                    .max(usize::from(aat::class::OUT_OF_BOUNDS) + 1)
                    .min(256);
                let rows = if classes == 0 {
                    0
                } else {
                    machine
                        .header
                        .state_array()
                        .ok()
                        .map_or(0, |array| array.data().len().div_ceil(classes).min(65536))
                };
                cost.classes = cost.classes.checked_add(wouldbe)?;
                cost.tails = cost
                    .tails
                    .checked_add(rows.saturating_sub(64).div_ceil(64))?;
                let mut visits = 64usize
                    .checked_add(wouldbe)?
                    .checked_add(rows.saturating_sub(64))?;
                let first = if let Ok(table) = machine.header.class_table() {
                    let count = table.class_array().len();
                    visits = visits.checked_add(classes)?.checked_add(count)?;
                    cost.temporary = cost.temporary.max(set_bytes(classes.div_ceil(PAGE_BITS))?);
                    // A 16-bit first glyph + 16-bit count, plus DELETED_GLYPH.
                    let pages = if count == 0 {
                        0
                    } else {
                        range_pages(usize::from(table.first_glyph()), count)?
                    };
                    pages.checked_add(1)?
                } else {
                    0
                };
                (first, 0, visits)
            }
            SubtableKind::Format2(table) => {
                let first = range_pages(
                    table.left_offset_table.first_glyph().to_u32() as usize,
                    usize::from(table.left_offset_table.n_glyphs()),
                )?;
                let second = range_pages(
                    table.right_offset_table.first_glyph().to_u32() as usize,
                    usize::from(table.right_offset_table.n_glyphs()),
                )?;
                (first, second, first.checked_add(second)?)
            }
            SubtableKind::Format3(table) => {
                let pages = range_pages(0, usize::from(table.glyph_count()))?;
                (pages, pages, pages.checked_mul(2)?)
            }
        };
        // set_digest visits the first sparse set's ranges. Charging every bit
        // of every occupied page bounds alternating singleton ranges too.
        budget
            .work(visits.checked_add(first.checked_mul(PAGE_BITS)?)?)
            .ok()?;
        cost.entries = cost.entries.checked_add(1)?;
        cost.boxes = cost.boxes.checked_add(size_of::<ClassCache>())?;
        cost.sets = cost
            .sets
            .checked_add(set_bytes(first)?)?
            .checked_add(set_bytes(second)?)?;
        let next = cost.peak()?;
        budget.alloc::<u8>(next.checked_sub(peak)?).ok()?;
        peak = next;
    }
    (cost.entries == expected).then_some(peak)
}
