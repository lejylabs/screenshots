//! Only cli::number and cli::text are called; never cli::run or command handlers.
use super::{number, text};
use std::ffi::OsString;
#[path = "../../../crates/app-core/tests/support/structured_parser_mutation.rs"]
mod mutation;

#[test]
fn structured_parser_fuzz_cli_numeric_tokens() {
    for (token, expected) in [
        ("0", 0),
        ("1", 1),
        ("4096", 4096),
        ("+1", 1),
        ("4294967295", u32::MAX),
    ] {
        assert!(
            number::<u32>(&OsString::from(token)).unwrap() == expected,
            "fixed u32 answer changed"
        );
    }
    for token in [
        "-1",
        "4294967296",
        "18446744073709551616",
        "",
        " 1",
        "1 ",
        "1.0",
        "NaN",
        "１２",
    ] {
        assert!(
            number::<u32>(&OsString::from(token))
                .unwrap_err()
                .to_string()
                == "Invalid numeric argument"
        );
    }
    for (token, expected) in [
        ("-1", -1),
        ("1", 1),
        ("4096", 4096),
        ("-9223372036854775808", i64::MIN),
        ("9223372036854775807", i64::MAX),
    ] {
        assert!(
            number::<i64>(&OsString::from(token)).unwrap() == expected,
            "fixed signed answer changed"
        );
    }
    for token in ["-9223372036854775809", "9223372036854775808"] {
        assert!(number::<i64>(&OsString::from(token)).is_err());
    }
    for (token, expected) in [("0", 0), ("1", 1), ("255", u8::MAX), ("+255", u8::MAX)] {
        assert!(
            number::<u8>(&OsString::from(token)).unwrap() == expected,
            "fixed narrow answer changed"
        );
    }
    for token in ["-1", "256", "4294967295"] {
        assert!(number::<u8>(&OsString::from(token)).is_err());
    }
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
        assert!(
            first
                == value
                    .parse::<u32>()
                    .map_err(|_| "Invalid numeric argument".to_owned()),
            "numeric u32 value differs from FromStr contract"
        );
        let first = number::<i64>(&argument).map_err(|error| error.to_string());
        assert!(first == number::<i64>(&argument).map_err(|error| error.to_string()));
        assert!(
            first
                == value
                    .parse::<i64>()
                    .map_err(|_| "Invalid numeric argument".to_owned()),
            "numeric i64 value differs from FromStr contract"
        );
        let first = number::<u8>(&argument).map_err(|error| error.to_string());
        assert!(first == number::<u8>(&argument).map_err(|error| error.to_string()));
        assert!(
            first
                == value
                    .parse::<u8>()
                    .map_err(|_| "Invalid numeric argument".to_owned()),
            "numeric u8 value differs from FromStr contract"
        );
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
