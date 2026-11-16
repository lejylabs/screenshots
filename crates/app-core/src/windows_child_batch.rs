// Batch argument serialization adapted from Rust 1.100.0 std/sys/args/windows.rs.
// Copyright (c) The Rust Project Contributors. MIT; full notice in THIRD_PARTY_NOTICES.md.
// https://github.com/rust-lang/rust/blob/1.100.0/library/std/src/sys/args/windows.rs
use super::*;

pub(super) fn user_path(path: &Path) -> Result<Vec<u16>> {
    let mut source = wide(path.as_os_str(), COMMAND_UNITS)?;
    if source.starts_with(&"\\\\?\\".encode_utf16().collect::<Vec<_>>()) {
        // cmd cannot consume verbatim prefixes. Strip only a short equivalent path,
        // using the same full-path equivalence check as the stable standard library.
        if source.len() <= 260 {
            let candidate = if source.get(5) == Some(&(b':' as u16)) {
                Some(source[4..].to_vec())
            } else if source.starts_with(&"\\\\?\\UNC\\".encode_utf16().collect::<Vec<_>>()) {
                let mut candidate = vec![b'\\' as u16, b'\\' as u16];
                candidate.extend_from_slice(&source[8..]);
                Some(candidate)
            } else {
                None
            };
            if let Some(candidate) = candidate {
                let result = full_path(&candidate)?;
                if result == candidate {
                    source = result;
                }
            }
        }
        Ok(source)
    } else {
        full_path(&source)
    }
}
fn full_path(source: &[u16]) -> Result<Vec<u16>> {
    let mut result = vec![0u16; COMMAND_UNITS + 1];
    let size = unsafe {
        GetFullPathNameW(
            source.as_ptr(),
            result.len() as u32,
            result.as_mut_ptr(),
            null_mut(),
        )
    } as usize;
    ensure!(
        size > 0 && size < result.len(),
        "Invalid native executable full path"
    );
    result.truncate(size + 1);
    Ok(result)
}
fn append(command: &mut Vec<u16>, value: &OsStr) -> Result<()> {
    let value = wide(value, COMMAND_UNITS)?;
    ensure!(
        !value.contains(&(b'\r' as u16)) && !value.contains(&(b'\n' as u16)),
        "Batch file arguments may not contain newlines"
    );
    let value = &value[..value.len() - 1];
    let unquoted = r"#$*+-./:?@\_";
    let quoted = value.is_empty()
        || value.last() == Some(&(b'\\' as u16))
        || char::decode_utf16(value.iter().copied())
            .flatten()
            .any(|character| {
                character.is_control()
                    || character.is_ascii()
                        && !(character.is_ascii_alphanumeric() || unquoted.contains(character))
            });
    if quoted {
        command.push(b'"' as u16);
    }
    let mut slashes = 0;
    for unit in value.iter().copied() {
        if unit == b'\\' as u16 {
            slashes += 1;
        } else {
            if unit == b'"' as u16 {
                command.extend(std::iter::repeat_n(b'\\' as u16, slashes));
                command.push(b'"' as u16);
            } else if unit == b'%' as u16 {
                command.extend("%%cd:~,".encode_utf16());
            }
            slashes = 0;
        }
        command.push(unit);
    }
    if quoted {
        command.extend(std::iter::repeat_n(b'\\' as u16, slashes));
        command.push(b'"' as u16);
    }
    ensure!(
        command.len() < COMMAND_UNITS,
        "Native batch command line exceeds its limit"
    );
    Ok(())
}
pub(super) fn command_line(script: &[u16], command: &Command) -> Result<Vec<u16>> {
    let script = script.strip_suffix(&[0]).unwrap_or(script);
    ensure!(
        !script.contains(&0)
            && !script.contains(&(b'"' as u16))
            && script.last() != Some(&(b'\\' as u16)),
        "Invalid native batch file name"
    );
    let mut result = "cmd.exe /e:ON /v:OFF /d /c \"\""
        .encode_utf16()
        .collect::<Vec<_>>();
    result.extend_from_slice(script);
    result.push(b'"' as u16);
    for value in command.get_args() {
        result.push(b' ' as u16);
        append(&mut result, value)?;
    }
    result.push(b'"' as u16);
    ensure!(
        result.len() < COMMAND_UNITS,
        "Native batch command line exceeds its limit"
    );
    Ok(result)
}
