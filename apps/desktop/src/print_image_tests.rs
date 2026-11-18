//! All process fixtures are owned mocks; these tests never invoke lp or GDI.
use super::*;

fn args(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}
fn valid() -> Vec<OsString> {
    #[cfg(unix)]
    return args(&[
        "example.png",
        "--printer",
        "private-fixture",
        "--paper",
        "a4",
        "--server",
        "127.0.0.1:631",
    ]);
    #[cfg(target_os = "windows")]
    return args(&[
        "example.png",
        "--printer",
        "Private fixture",
        "--paper",
        "a4",
    ]);
}
#[test]
fn destination_and_paper_are_always_explicit() {
    assert!(parse(&valid()).is_ok());
    for input in [
        vec![],
        args(&["example.png"]),
        args(&["example.png", "--paper", "a4"]),
        args(&["example.png", "--printer", "private-fixture"]),
    ] {
        assert!(parse(&input).is_err());
    }
    let mut duplicate = valid();
    duplicate.extend(args(&["--paper", "letter"]));
    assert!(parse(&duplicate).is_err());
    let mut unknown = valid();
    unknown[3] = "--copies".into();
    assert!(parse(&unknown).is_err());
    let mut control = valid();
    control[2] = "printer\nsecret".into();
    assert!(parse(&control).is_err());
}
#[test]
fn cups_server_never_accepts_credentials_paths_or_options() {
    for server in [
        "",
        "-h:631",
        "user:secret@host:631",
        "https://host:631",
        "host/queue:631",
        "host:0",
        "host:65536",
        "host:631\n",
        "::1",
        "host",
    ] {
        assert!(validate_server(server).is_err(), "{server}");
    }
    assert!(validate_server("127.0.0.1:34567").is_ok());
    assert!(validate_server("print.example.test:631").is_err());
    assert!(validate_server("192.0.2.10:631").is_err());
    assert!(validate_server("127.attacker.test:631").is_err());
}
#[cfg(unix)]
fn queue_response(name: &str) -> Vec<u8> {
    let mut response = vec![1, 1, 0, 0, 0, 0, 0, 1, 4];
    ipp_attribute(&mut response, 0x42, "printer-name", name.as_bytes());
    ipp_attribute(&mut response, 0x22, "printer-is-temporary", &[0]);
    ipp_attribute(&mut response, 0x22, "printer-is-accepting-jobs", &[1]);
    ipp_attribute(
        &mut response,
        0x49,
        "document-format-supported",
        b"application/postscript",
    );
    ipp_attribute(
        &mut response,
        0x23,
        "operations-supported",
        &2i32.to_be_bytes(),
    );
    response.push(3);
    response
}
#[cfg(unix)]
#[test]
fn ipp_admits_only_the_exact_existing_queue() -> Result<()> {
    let response = queue_response("private-fixture");
    ipp_existing_queue(&response, "private-fixture")?;
    assert!(ipp_existing_queue(&response, "other").is_err());
    for length in 0..response.len() {
        assert!(ipp_existing_queue(&response[..length], "private-fixture").is_err());
    }
    let mut absent = response.clone();
    absent[2..4].copy_from_slice(&0x0406u16.to_be_bytes());
    assert!(ipp_existing_queue(&absent, "private-fixture").is_err());
    let mut substituted = response.clone();
    substituted[3] = 1;
    assert!(ipp_existing_queue(&substituted, "private-fixture").is_err());
    let mut ambiguous = response[..response.len() - 1].to_vec();
    ipp_attribute(&mut ambiguous, 0x42, "printer-name", b"private-fixture");
    ambiguous.push(3);
    assert!(ipp_existing_queue(&ambiguous, "private-fixture").is_err());
    let mut temporary = response[..response.len() - 1].to_vec();
    ipp_attribute(&mut temporary, 0x22, "printer-is-temporary", &[1]);
    temporary.push(3);
    assert!(ipp_existing_queue(&temporary, "private-fixture").is_err());
    assert!(ipp_response(&vec![0; IPP_RESPONSE_LIMIT + 1], 1).is_err());
    Ok(())
}
#[cfg(unix)]
#[test]
fn ipp_submission_has_fixed_options_and_no_source_metadata() -> Result<()> {
    let options = parse(&valid())?;
    let request = ipp_print_request(
        "ipp://127.0.0.1:631/printers/private-fixture",
        options.paper,
    );
    let mut response = request;
    assert_eq!(&response[2..4], &2u16.to_be_bytes());
    // Use the bounded attribute parser to inspect the request's fixed wire attributes.
    response[2..4].copy_from_slice(&[0, 0]);
    let attributes = ipp_response(&response, 2)?;
    assert_eq!(ipp_one(&attributes, 1, 0x42, b"job-name")?, b"Screenshot");
    assert_eq!(
        ipp_one(&attributes, 1, 0x22, b"ipp-attribute-fidelity")?,
        &[1]
    );
    assert_eq!(
        ipp_one(&attributes, 2, 0x21, b"copies")?,
        &1i32.to_be_bytes()
    );
    assert_eq!(
        ipp_one(&attributes, 2, 0x21, b"number-up")?,
        &1i32.to_be_bytes()
    );
    assert_eq!(
        ipp_one(&attributes, 2, 0x23, b"orientation-requested")?,
        &3i32.to_be_bytes()
    );
    assert_eq!(
        ipp_one(&attributes, 2, 0x44, b"media")?,
        b"iso_a4_210x297mm"
    );
    assert_eq!(ipp_one(&attributes, 2, 0x44, b"job-sheets")?, b"none");
    assert_eq!(ipp_one(&attributes, 2, 0x44, b"print-scaling")?, b"none");
    assert!(!response.windows(11).any(|bytes| bytes == b"example.png"));
    for printer in ["-d", "queue/instance", "queue name", "queue@server"] {
        let mut input = valid();
        input[2] = printer.into();
        assert!(parse(&input).is_err());
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn spool_http_is_bounded_and_never_follows_redirects_or_authentication() -> Result<()> {
    let body = queue_response("private-fixture");
    let response = |headers: &str, body: &[u8]| {
        let mut bytes = headers.as_bytes().to_vec();
        bytes.extend_from_slice(body);
        bytes
    };
    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/ipp\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    assert_eq!(
        ipp_http_response(std::io::Cursor::new(response(&header, &body)))?,
        body
    );
    for headers in [
        "HTTP/1.1 302 Found\r\nLocation: http://192.0.2.1/\r\n\r\n",
        "HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n",
        "HTTP/1.1 200 OK\r\nContent-Type: application/ipp\r\nContent-Length: 9\r\nContent-Length: 9\r\n\r\n",
        "HTTP/1.1 200 OK\r\nContent-Type: application/ipp\r\nTransfer-Encoding: chunked\r\n\r\n",
        "HTTP/1.1 200 OK\r\nContent-Type: application/ipp\r\nContent-Length: 65537\r\n\r\n",
        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 9\r\n\r\n",
        "HTTP/1.1 200 OK\r\nContent-Type: application/ipp\r\n\r\n",
    ] {
        assert!(ipp_http_response(std::io::Cursor::new(response(headers, &body))).is_err());
    }
    assert!(ipp_http_response(std::io::Cursor::new(vec![b'x'; 8193])).is_err());
    assert!(
        ipp_http_response(std::io::Cursor::new(response(
            &header,
            &body[..body.len() - 1]
        )))
        .is_err()
    );
    Ok(())
}

#[cfg(unix)]
fn mock(mode: &str) -> Command {
    if std::env::var_os("SCREENSHOTS_PRINT_TEST_TRACE").is_some() {
        eprintln!("OWNED print mock mode={mode}");
    }
    let mut command = Command::new("/usr/bin/python3");
    command
        .arg(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/print_image/fake_worker.py"),
        )
        .arg(mode)
        .env_clear()
        .env("LC_ALL", "C");
    command
}

#[cfg(unix)]
#[test]
fn owned_worker_acceptance_rejection_timeout_and_cancel() -> Result<()> {
    // Serialized inside one test: admission deliberately rejects concurrent print attempts.
    supervise(
        mock("accept"),
        &|| false,
        Instant::now() + Duration::from_secs(5),
    )?;
    let error = supervise(
        mock("reject"),
        &|| false,
        Instant::now() + Duration::from_secs(5),
    )
    .unwrap_err();
    assert!(error.to_string().contains("before submission"));
    for mode in ["unknown", "missing-ack", "wrong-ack", "extra-descendant"] {
        assert!(
            supervise(
                mock(mode),
                &|| false,
                Instant::now() + Duration::from_secs(5)
            )
            .is_err(),
            "{mode}"
        );
    }
    let error = supervise(
        mock("hang"),
        &|| false,
        Instant::now() + Duration::from_millis(100),
    )
    .unwrap_err();
    assert!(error.to_string().contains("timed out"));
    let until = Instant::now() + Duration::from_millis(50);
    let error = supervise(
        mock("hang"),
        &|| Instant::now() >= until,
        Instant::now() + Duration::from_secs(5),
    )
    .unwrap_err();
    assert!(error.to_string().contains("canceled"));
    // A new command can run after every owned child/group has been released.
    supervise(
        mock("accept"),
        &|| false,
        Instant::now() + Duration::from_secs(5),
    )?;
    Ok(())
}

#[cfg(target_os = "windows")]
#[test]
#[ignore = "requires explicitly compiled owned fake_worker_windows.rs and SCREENSHOTS_PRINT_TEST_HELPER"]
fn windows_owned_job_acceptance_rejection_timeout_and_cancel() -> Result<()> {
    let path = std::env::var_os("SCREENSHOTS_PRINT_TEST_HELPER")
        .context("Compile the owned Windows mock fixture and set SCREENSHOTS_PRINT_TEST_HELPER")?;
    ensure!(
        std::path::Path::new(&path).is_absolute(),
        "Fixture path must be absolute"
    );
    let mock = |mode: &str| {
        let mut command = Command::new(&path);
        command.arg(mode).env_clear();
        for key in ["SystemRoot", "WINDIR"] {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        command
    };
    for mode in ["accept", "attempt-child"] {
        supervise(
            mock(mode),
            &|| false,
            Instant::now() + Duration::from_secs(5),
        )?;
    }
    assert!(
        supervise(
            mock("reject"),
            &|| false,
            Instant::now() + Duration::from_secs(5)
        )
        .unwrap_err()
        .to_string()
        .contains("before submission")
    );
    for mode in ["unknown", "missing-ack", "wrong-ack"] {
        assert!(
            supervise(
                mock(mode),
                &|| false,
                Instant::now() + Duration::from_secs(5)
            )
            .is_err()
        );
    }
    assert!(
        supervise(
            mock("hang"),
            &|| false,
            Instant::now() + Duration::from_millis(100)
        )
        .unwrap_err()
        .to_string()
        .contains("timed out")
    );
    let until = Instant::now() + Duration::from_millis(50);
    assert!(
        supervise(
            mock("hang"),
            &|| Instant::now() >= until,
            Instant::now() + Duration::from_secs(5)
        )
        .unwrap_err()
        .to_string()
        .contains("canceled")
    );
    supervise(
        mock("accept"),
        &|| false,
        Instant::now() + Duration::from_secs(5),
    )?;
    Ok(())
}
