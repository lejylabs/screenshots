//! Deterministic, bounded mutation smoke; this is not a coverage-guided engine.
use std::panic::{AssertUnwindSafe, catch_unwind};

pub const SEED: u64 = 0x5c72_2714_a11c_e905;
pub const CASES: usize = 2048;
pub const MAX_INPUT: usize = 16 * 1024;

pub struct Random(pub u64);
impl Random {
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    pub fn index(&mut self, length: usize) -> usize {
        (self.next() as usize) % length.max(1)
    }
}

fn mutate(source: &[u8], index: usize, random: &mut Random) -> Vec<u8> {
    const TOKENS: &[&[u8]] = &[
        b"0",
        b"-1",
        b"4294967296",
        b"18446744073709551616",
        b"1e309",
        b"NaN",
        b"null",
        b"\\u0000",
        b"\\ud800",
        b"\0",
        b"\xff",
        b"\\",
        b"\"",
        b"{",
        b"]",
        "🦊".as_bytes(),
        "\u{2028}".as_bytes(),
        b"../",
        b"%2f",
        b"&amp;",
        b"{json:a~1b}",
        b"(?P<url>.+)",
        b"<!DOCTYPE x [<!ENTITY e SYSTEM 'file:///fixture-only'>]>",
    ];
    let mut bytes = source.to_vec();
    let at = random.index(bytes.len() + 1);
    let token = TOKENS[random.index(TOKENS.len())];
    match index % 10 {
        0 => {}
        1 => bytes.truncate(at),
        2 if !bytes.is_empty() => {
            bytes.remove(at.min(bytes.len() - 1));
        }
        3 => {
            bytes.splice(at..at, token.iter().copied());
        }
        4 if !bytes.is_empty() => {
            let end = (at + token.len()).min(bytes.len());
            bytes.splice(at..end, token.iter().copied());
        }
        5 if !bytes.is_empty() => {
            let end = random.index(bytes.len());
            bytes.swap(at.min(bytes.len() - 1), end);
        }
        6 => bytes.extend_from_slice(token),
        7 => {
            bytes.splice(0..0, token.iter().copied());
        }
        8 => {
            if let Some(start) = bytes.iter().position(u8::is_ascii_digit) {
                let end = start
                    + bytes[start..]
                        .iter()
                        .take_while(|byte| byte.is_ascii_digit())
                        .count();
                bytes.splice(start..end, token.iter().copied());
            }
        }
        9 if !bytes.is_empty() => {
            let start = random.index(bytes.len());
            let end = (start + random.index(65)).min(bytes.len());
            let fragment = bytes[start..end].to_vec();
            bytes.splice(at..at, fragment);
        }
        _ => {}
    }
    bytes.truncate(MAX_INPUT);
    bytes
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

// At most 64 deletion attempts; report an irreducible-within-budget synthetic witness.
fn reduce(mut bytes: Vec<u8>, check: &impl Fn(&[u8]) -> usize) -> Vec<u8> {
    let mut chunk = bytes.len().div_ceil(2);
    let mut attempts = 0;
    while chunk > 0 && attempts < 64 {
        let mut start = 0;
        let mut changed = false;
        while start < bytes.len() && attempts < 64 {
            let mut candidate = bytes.clone();
            candidate.drain(start..(start + chunk).min(bytes.len()));
            attempts += 1;
            if catch_unwind(AssertUnwindSafe(|| check(&candidate))).is_err() {
                bytes = candidate;
                changed = true;
            } else {
                start += chunk;
            }
        }
        if !changed {
            chunk /= 2;
        }
    }
    bytes
}

pub fn run(family: &str, corpus: &[Vec<u8>], check: impl Fn(&[u8]) -> usize) {
    assert!(!corpus.is_empty());
    assert!(corpus.iter().all(|value| value.len() <= MAX_INPUT));
    let started = std::time::Instant::now();
    let mut random = Random(SEED);
    let mut max_input = 0;
    let mut max_output = 0;
    let mut mutations = [0_usize; 10];
    for index in 0..(corpus.len() + CASES) {
        let bytes = if index < corpus.len() {
            corpus[index].clone()
        } else {
            let mutation = index - corpus.len();
            mutations[mutation % 10] += 1;
            mutate(&corpus[random.index(corpus.len())], mutation, &mut random)
        };
        max_input = max_input.max(bytes.len());
        match catch_unwind(AssertUnwindSafe(|| check(&bytes))) {
            Ok(output) => max_output = max_output.max(output),
            Err(_) => {
                let reduced = reduce(bytes.clone(), &check);
                panic!(
                    "structured fuzz family={family} seed={SEED:#x} case={index} original_hex={} reduced_hex={} (64-attempt deletion budget)",
                    hex(&bytes),
                    hex(&reduced)
                );
            }
        }
    }
    eprintln!(
        "STRUCTURED_FUZZ {{\"family\":\"{family}\",\"seed\":\"{SEED:#x}\",\"corpus\":{},\"mutations\":{mutations:?},\"cases\":{},\"max_input\":{max_input},\"max_output\":{max_output},\"elapsed_us\":{}}}",
        corpus.len(),
        corpus.len() + CASES,
        started.elapsed().as_micros()
    );
}

pub fn lines(text: &str) -> Vec<Vec<u8>> {
    text.lines()
        .filter(|line| !line.is_empty())
        .map(|line| line.as_bytes().to_vec())
        .collect()
}
