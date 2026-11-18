//! Credential bytes use only the inherited authenticated channel, never capture serde.
use super::*;
use app_core::NativeCredentialCall;
use std::io::{Read, Write};
use zeroize::Zeroizing;

const ARGUMENT: &str = "__native-credential-worker";
const TIMEOUT: Duration = Duration::from_secs(5);
const MAGIC: &[u8; 8] = b"SCCRED01";
const MAX_TARGET: usize = 4096;
const MAX_PAYLOAD: usize = 8192;

pub(super) fn install() -> Result<()> {
    app_core::install_native_credential_executor(execute)
}
fn execute(
    call: NativeCredentialCall,
    check: &NativeCaptureCheck<'_>,
) -> Result<Zeroizing<Vec<u8>>> {
    execute_factory(call, check, TIMEOUT, || {
        super::command_for(ARGUMENT)
            .map_err(|_| CaptureError::Backend("Native credential executable unavailable".into()))
    })
    .map_err(Into::into)
}
fn execute_factory(
    call: NativeCredentialCall,
    check: &NativeCaptureCheck<'_>,
    timeout: Duration,
    factory: impl FnOnce() -> Result<Command, CaptureError> + Send + 'static,
) -> Result<Zeroizing<Vec<u8>>, CaptureError> {
    call.validate().map_err(|_| invalid())?;
    super::execute_owned(
        super::AdmissionPriority::OptionalCredentials,
        check,
        timeout,
        move |deadline, cancel, owned| {
            let check = || {
                if Instant::now() >= deadline {
                    Err(CaptureError::NativeTimeout)
                } else if cancel.load(Ordering::Acquire) {
                    Err(CaptureError::NativeCanceled)
                } else {
                    Ok(())
                }
            };
            let result = (|| -> Result<Zeroizing<Vec<u8>>> {
                check()?;
                let mut command = factory()?;
                super::private_environment(&mut command);
                let prepared = transport::Prepared::new(&mut command)?;
                check()?;
                let job = super::launch(&mut command, &prepared)?;
                *super::lock(&owned) = Some(job);
                let (transport, nonce) = prepared.parent();
                let mut io = transport::ActiveIo {
                    transport,
                    deadline,
                    check: &check,
                };
                protocol::handshake(&mut io, protocol::HELLO, &nonce)?;
                protocol::expect_handshake(&mut io, protocol::ACK, &nonce)?;
                write_request(&mut io, &call)?;
                read_response(&mut io, &call)
            })()
            .map_err(|_| check().err().unwrap_or_else(invalid));
            let result = if let Err(error) = check() {
                Err(error)
            } else {
                result
            };
            super::finish_owned(result, &owned)
        },
    )
}
fn invalid() -> CaptureError {
    CaptureError::Backend("Native credential helper failed or returned invalid data".into())
}
fn write_request(io: &mut impl Write, call: &NativeCredentialCall) -> Result<()> {
    call.validate()?;
    let (kind, payload): (u8, &[u8]) = match call {
        NativeCredentialCall::Read { .. } => (1, &[]),
        NativeCredentialCall::Write { payload, .. } => (2, payload),
        NativeCredentialCall::Remove { .. } => (3, &[]),
    };
    io.write_all(MAGIC)?;
    io.write_all(&[kind])?;
    io.write_all(&(call.target().len() as u32).to_be_bytes())?;
    io.write_all(&(payload.len() as u32).to_be_bytes())?;
    io.write_all(call.target().as_bytes())?;
    io.write_all(payload)?;
    io.flush()?;
    Ok(())
}
fn read_request(io: &mut impl Read) -> Result<NativeCredentialCall> {
    let mut header = [0u8; 17];
    io.read_exact(&mut header)?;
    ensure!(&header[..8] == MAGIC, "Invalid credential protocol");
    let target_len = u32::from_be_bytes(header[9..13].try_into().unwrap()) as usize;
    let payload_len = u32::from_be_bytes(header[13..17].try_into().unwrap()) as usize;
    ensure!(
        (1..=MAX_TARGET).contains(&target_len) && payload_len <= MAX_PAYLOAD,
        "Invalid credential request sizes"
    );
    ensure!(
        matches!(header[8], 1..=3) && (header[8] == 2 || payload_len == 0),
        "Invalid credential operation"
    );
    let mut target = Zeroizing::new(vec![0u8; target_len]);
    io.read_exact(&mut target)?;
    let target = std::str::from_utf8(&target)
        .map_err(|_| anyhow::anyhow!("Invalid credential target encoding"))?
        .to_owned();
    let mut payload = Zeroizing::new(vec![0u8; payload_len]);
    io.read_exact(&mut payload)?;
    let call = match header[8] {
        1 => NativeCredentialCall::Read { target },
        2 => NativeCredentialCall::Write { target, payload },
        3 => NativeCredentialCall::Remove { target },
        _ => unreachable!(),
    };
    call.validate()?;
    Ok(call)
}
fn read_response(io: &mut impl Read, call: &NativeCredentialCall) -> Result<Zeroizing<Vec<u8>>> {
    let mut header = [0u8; 13];
    io.read_exact(&mut header)?;
    ensure!(
        &header[..8] == MAGIC && header[8] <= 1,
        "Invalid credential response"
    );
    let length = u32::from_be_bytes(header[9..13].try_into().unwrap()) as usize;
    ensure!(
        length <= call.payload_limit(),
        "Credential response exceeds its limit"
    );
    ensure!(
        header[8] == 0 && (matches!(call, NativeCredentialCall::Read { .. }) || length == 0),
        "Credential operation was not confirmed"
    );
    let mut payload = Zeroizing::new(vec![0u8; length]);
    io.read_exact(&mut payload)?;
    Ok(payload)
}
pub(super) fn try_child(arguments: &[OsString]) -> Result<bool> {
    if arguments.first().is_none_or(|arg| arg != ARGUMENT) {
        return Ok(false);
    }
    ensure!(
        arguments.len() == 1,
        "Invalid private credential worker arguments"
    );
    serve().map_err(|_| anyhow::anyhow!("Private native credential worker failed"))?;
    Ok(true)
}
fn serve() -> Result<()> {
    let (transport, nonce, parent) = transport::Transport::inherited()?;
    #[cfg(target_os = "macos")]
    let _lifeline = transport::Lifeline::new(parent)?;
    #[cfg(not(target_os = "macos"))]
    let _ = parent;
    let mut io = transport::ActiveIo {
        transport,
        deadline: Instant::now() + TIMEOUT,
        check: &|| Ok(()),
    };
    protocol::expect_handshake(&mut io, protocol::HELLO, &nonce)?;
    protocol::handshake(&mut io, protocol::ACK, &nonce)?;
    let call = read_request(&mut io)?;
    let limit = call.payload_limit();
    let read = matches!(call, NativeCredentialCall::Read { .. });
    let result = call.run_in_owned_child();
    // Only the success discriminator and bounded bytes cross back. Native error text is discarded.
    let success = result
        .as_ref()
        .is_ok_and(|payload| payload.len() <= limit && (read || payload.is_empty()));
    let payload = if success {
        result.unwrap()
    } else {
        Zeroizing::new(Vec::new())
    };
    io.write_all(MAGIC)?;
    io.write_all(&[u8::from(!success)])?;
    io.write_all(&(payload.len() as u32).to_be_bytes())?;
    io.write_all(&payload)?;
    io.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    #[test]
    fn secret_protocol_roundtrip_bounds_and_sanitized_failures() {
        let call = NativeCredentialCall::Write {
            target: "Screenshots/upload/https://example.invalid/synthetic".into(),
            payload: Zeroizing::new(b"synthetic-private".to_vec()),
        };
        let mut wire = Zeroizing::new(Vec::new());
        write_request(&mut *wire, &call).unwrap();
        let parsed = read_request(&mut Cursor::new(&*wire)).unwrap();
        assert!(
            matches!(parsed, NativeCredentialCall::Write { payload, .. } if payload.as_slice() == b"synthetic-private")
        );
        let mut hostile = [0u8; 17];
        hostile[..8].copy_from_slice(MAGIC);
        hostile[8] = 2;
        hostile[9..13].copy_from_slice(&4097u32.to_be_bytes());
        assert!(read_request(&mut Cursor::new(hostile)).is_err());
        let mut hostile = [0u8; 13];
        hostile[..8].copy_from_slice(MAGIC);
        hostile[8] = 1;
        let error = read_response(&mut Cursor::new(hostile), &call)
            .unwrap_err()
            .to_string();
        assert!(!error.contains("synthetic-private"));
    }
    #[cfg(target_os = "linux")]
    include!("credential_worker_linux_tests.rs");
    #[cfg(target_os = "linux")]
    include!("credential_worker_signal_tests.rs");
}
