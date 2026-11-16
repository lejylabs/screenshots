//! Synthetic legacy font fixtures; never installed or distributed as modified fonts.
pub fn font_with_legacy_kern(kern: &[u8]) -> Vec<u8> {
    fn checksum(bytes: &[u8]) -> u32 {
        bytes.chunks(4).fold(0u32, |sum, chunk| {
            let mut word = [0; 4];
            word[..chunk.len()].copy_from_slice(chunk);
            sum.wrapping_add(u32::from_be_bytes(word))
        })
    }
    let mut font =
        include_bytes!("../../../../vendor/cosmic-text/fonts/FiraMono-Medium.ttf").to_vec();
    while !font.len().is_multiple_of(4) {
        font.push(0);
    }
    let offset = font.len();
    font.extend_from_slice(kern);
    while !font.len().is_multiple_of(4) {
        font.push(0);
    }
    let count = u16::from_be_bytes(font[4..6].try_into().unwrap()) as usize;
    let mut records = Vec::new();
    let mut head = None;
    for index in 0..count {
        let mut record = font[12 + index * 16..28 + index * 16].to_vec();
        match &record[..4] {
            b"DSIG" => {
                record[..4].copy_from_slice(b"kern");
                record[4..8].copy_from_slice(&checksum(kern).to_be_bytes());
                record[8..12].copy_from_slice(&(offset as u32).to_be_bytes());
                record[12..16].copy_from_slice(&(kern.len() as u32).to_be_bytes());
            }
            // Ensure normal controls actually select AAT legacy kerning.
            b"GPOS" => record[..4].copy_from_slice(b"XPOS"),
            b"head" => head = Some(u32::from_be_bytes(record[8..12].try_into().unwrap()) as usize),
            _ => {}
        }
        records.push(record);
    }
    records.sort_by(|a, b| a[..4].cmp(&b[..4]));
    for (index, record) in records.iter().enumerate() {
        font[12 + index * 16..28 + index * 16].copy_from_slice(record);
    }
    let adjust = head.unwrap() + 8;
    font[adjust..adjust + 4].fill(0);
    let adjustment = 0xb1b0afbau32.wrapping_sub(checksum(&font));
    font[adjust..adjust + 4].copy_from_slice(&adjustment.to_be_bytes());
    assert_eq!(checksum(&font), 0xb1b0afba);
    font
}
pub fn apple_kern(format: u16, body: &[u8], count: u32) -> Vec<u8> {
    let mut table = 0x00010000u32.to_be_bytes().to_vec();
    table.extend_from_slice(&count.to_be_bytes());
    let length = 8 + body.len() as u32;
    for _ in 0..count {
        table.extend_from_slice(&length.to_be_bytes());
        table.extend_from_slice(&format.to_be_bytes());
        table.extend_from_slice(&0u16.to_be_bytes());
        table.extend_from_slice(body);
    }
    table
}
pub fn kern_words(words: impl IntoIterator<Item = u16>) -> Vec<u8> {
    words.into_iter().flat_map(u16::to_be_bytes).collect()
}
