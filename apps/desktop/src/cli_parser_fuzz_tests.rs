//! Only cli::number and cli::text are called; never cli::run or command handlers.
use super::{number, text};
use std::ffi::OsString;
#[path = "../../../crates/app-core/tests/support/structured_parser_mutation.rs"]
mod mutation;

#[test]
fn structured_parser_fuzz_cli_numeric_tokens() {
    let corpus = [
        "0",
        "1",
        "4096",
        "+1",
        "-1",
        "4294967295",
        "4294967296",
        "18446744073709551616",
        "",
        " 1",
        "1 ",
        "1.0",
        "1e9",
        "NaN",
        "Inf",
        "１２",
        "١",
        "1\0",
        "🦊",
        "2714-synthetic-private",
    ]
    .map(|text| text.as_bytes().to_vec());
    mutation::run("cli_numeric_tokens", &corpus, |bytes| {
        let Ok(value) = std::str::from_utf8(bytes) else {
            return 0;
        };
        let argument = OsString::from(value);
        let parse = || number::<u32>(&argument).map_err(|error| error.to_string());
        let first = parse();
        assert!(first == parse(), "numeric parsing changed");
        assert!(first.is_ok() == value.parse::<u32>().is_ok());
        if let Ok(value) = first {
            assert!(number::<u32>(&OsString::from(value.to_string())).unwrap() == value);
        } else {
            assert!(first.unwrap_err() == "Invalid numeric argument");
        }
        let first = number::<i64>(&argument).map_err(|error| error.to_string());
        assert!(first == number::<i64>(&argument).map_err(|error| error.to_string()));
        let first = number::<u8>(&argument).map_err(|error| error.to_string());
        assert!(first == number::<u8>(&argument).map_err(|error| error.to_string()));
        std::mem::size_of::<u32>()
    });
}

#[test]
fn structured_parser_fuzz_cli_non_utf8_tokens() {
    #[cfg(unix)]
    let invalid = {
        use std::os::unix::ffi::OsStringExt;
        OsString::from_vec(vec![b'1', 0xff, b'2'])
    };
    #[cfg(target_os = "windows")]
    let invalid = {
        use std::os::windows::ffi::OsStringExt;
        OsString::from_wide(&[0x31, 0xd800, 0x32])
    };
    #[cfg(any(unix, target_os = "windows"))]
    {
        assert!(text(&invalid).unwrap_err().to_string() == "Option must be valid UTF-8");
        assert!(number::<u32>(&invalid).unwrap_err().to_string() == "Option must be valid UTF-8");
    }
}
