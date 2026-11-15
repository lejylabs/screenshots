//! Explicit finite PCM audio capture through the existing isolated recorder engines.

use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;
use std::{
    ffi::OsString,
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
    process::Command,
    time::{Duration, Instant},
};

pub(crate) const RATE: u32 = 48_000;
const USAGE: &str = "Usage: screenshots record-audio test|pulse|alsa|dshow|avfoundation OUTPUT.wav --seconds N [--device NAME] [--channels 1|2]";

struct Options<'a> {
    mode: &'a str,
    destination: &'a Path,
    device: Option<&'a str>,
    seconds: u32,
    channels: u16,
}

#[derive(Clone, Copy)]
pub(crate) struct AudioInput<'a> {
    pub(crate) mode: &'a str,
    device: Option<&'a str>,
    pub(crate) channels: u16,
}

impl<'a> AudioInput<'a> {
    pub(crate) fn new(
        mode: &'a str,
        source: Option<&'a str>,
        channels: Option<u16>,
    ) -> Result<Self> {
        ensure!(
            mode == "test"
                || (cfg!(target_os = "linux") && matches!(mode, "pulse" | "alsa"))
                || (cfg!(target_os = "windows") && mode == "dshow")
                || (cfg!(target_os = "macos") && mode == "avfoundation"),
            "Audio input is unavailable on this platform"
        );
        let channels = channels.unwrap_or(2);
        ensure!(matches!(channels, 1 | 2), "Audio channels must be 1 or 2");
        if mode == "test" {
            ensure!(source.is_none(), "Synthetic audio does not accept a device");
        } else {
            device(mode, source.context("Native audio requires --device")?)?;
        }
        Ok(Self {
            mode,
            device: source,
            channels,
        })
    }

    pub(crate) fn append_input(&self, command: &mut Command) {
        command.args(["-thread_queue_size", "1"]);
        match self.mode {
            "test" => {
                command.args([
                    "-re",
                    "-f",
                    "lavfi",
                    "-i",
                    "sine=frequency=440:sample_rate=48000",
                ]);
            }
            "pulse" | "alsa" => {
                command.args([
                    "-f",
                    self.mode,
                    "-sample_rate",
                    "48000",
                    "-channels",
                    &self.channels.to_string(),
                    "-i",
                    self.device.unwrap(),
                ]);
            }
            "dshow" => {
                command.args([
                    "-f",
                    "dshow",
                    "-i",
                    &format!("audio={}", self.device.unwrap()),
                ]);
            }
            "avfoundation" => {
                command.args([
                    "-f",
                    "avfoundation",
                    "-i",
                    &format!("none:{}", self.device.unwrap()),
                ]);
            }
            _ => unreachable!("validated audio mode"),
        }
    }

    pub(crate) fn append_pcm(&self, command: &mut Command, seconds: u32, combined: bool) {
        let mut filter = format!(
            "aresample=48000,atrim=end_sample={},asetpts=N/SR/TB",
            seconds * RATE
        );
        if combined {
            filter.push_str(",asetnsamples=n=48000:p=0");
        }
        command.args([
            "-af",
            &filter,
            "-ar",
            "48000",
            "-ac",
            &self.channels.to_string(),
            "-c:a",
            "pcm_s16le",
            "-fflags",
            "+bitexact",
            "-flags:a",
            "+bitexact",
        ]);
    }

    pub(crate) fn summary(&self, seconds: u32) -> serde_json::Value {
        serde_json::json!({"mode":self.mode,"codec":"pcm_s16le","sample_rate":RATE,"channels":self.channels,"samples":seconds * RATE})
    }
}

impl Options<'_> {
    fn samples(&self) -> u32 {
        self.seconds * RATE
    }

    fn pcm_bytes(&self) -> u32 {
        self.samples() * u32::from(self.channels) * 2
    }
}

fn device(mode: &str, value: &str) -> Result<()> {
    ensure!(
        !value.is_empty()
            && value.len() <= 256
            && value.trim() == value
            && !value.chars().any(char::is_control),
        "Audio device must contain 1–256 UTF-8 bytes without controls or edge whitespace"
    );
    let valid = match mode {
        "pulse" => value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_.@-".contains(&byte)),
        "alsa" => value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_.@-:,=".contains(&byte)),
        "dshow" => !value.contains([':', '=', '"']),
        "avfoundation" => {
            value == "default"
                || (value.len() <= 10
                    && value.bytes().all(|byte| byte.is_ascii_digit())
                    && value.parse::<u32>().is_ok())
        }
        _ => false,
    };
    ensure!(
        valid,
        "Audio device syntax is invalid for the selected input"
    );
    Ok(())
}

fn options(args: &[OsString]) -> Result<Options<'_>> {
    ensure!(matches!(args.len(), 4 | 6 | 8), USAGE);
    let mode = args[0].to_str().context("Audio mode must be UTF-8")?;
    ensure!(
        args[1].as_encoded_bytes().len() <= 4096,
        "Audio output path exceeds 4096 bytes"
    );
    let destination = Path::new(&args[1]);
    ensure!(
        destination
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("wav")),
        "Audio output must have a .wav extension"
    );
    match destination.symlink_metadata() {
        Ok(_) => bail!("Audio output already exists"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => bail!("Audio output cannot be inspected"),
    }
    let mut seconds = None;
    let mut channels = None;
    let mut source = None;
    for pair in args[2..].chunks_exact(2) {
        match pair[0].to_str() {
            Some("--seconds") => {
                ensure!(seconds.is_none(), "Duplicate --seconds option");
                let value = pair[1].to_str().context("Audio seconds must be UTF-8")?;
                seconds = Some(
                    value
                        .parse::<u32>()
                        .context("Audio seconds must be a whole number")?,
                );
            }
            Some("--channels") => {
                ensure!(channels.is_none(), "Duplicate --channels option");
                let value = pair[1].to_str().context("Audio channels must be UTF-8")?;
                channels = Some(
                    value
                        .parse::<u16>()
                        .context("Audio channels must be a whole number")?,
                );
            }
            Some("--device") => {
                ensure!(source.is_none(), "Duplicate --device option");
                source = Some(pair[1].to_str().context("Audio device must be UTF-8")?);
            }
            _ => bail!("Unknown audio recording option; use --help"),
        }
    }
    let seconds = seconds.context("Missing --seconds")?;
    ensure!((1..=300).contains(&seconds), "Audio seconds must be 1–300");
    let input = AudioInput::new(mode, source, channels)?;
    Ok(Options {
        mode,
        destination,
        device: source,
        seconds,
        channels: input.channels,
    })
}

fn encoder(options: &Options<'_>, stage: &Path) -> Command {
    let mut command = Command::new(if cfg!(target_os = "windows") {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    });
    command.args([
        "-hide_banner",
        "-loglevel",
        "error",
        "-max_alloc",
        "67108864",
        "-nostdin",
        "-n",
        "-filter_threads",
        "1",
        "-filter_complex_threads",
        "1",
        "-threads",
        "2",
    ]);
    let input = AudioInput {
        mode: options.mode,
        device: options.device,
        channels: options.channels,
    };
    input.append_input(&mut command);
    command.args([
        "-map",
        "0:a:0",
        "-vn",
        "-sn",
        "-dn",
        "-map_metadata",
        "-1",
        "-map_chapters",
        "-1",
        "-t",
        &options.seconds.to_string(),
    ]);
    input.append_pcm(&mut command, options.seconds, false);
    command.args(["-threads", "2", "-max_muxing_queue_size", "1", "-f", "wav"]);
    if cfg!(target_os = "windows") {
        command.arg("pipe:1");
    } else {
        command.arg(stage);
    }
    command
}

fn probe(stage: &Path) -> Command {
    let mut command = Command::new(if cfg!(target_os = "windows") {
        "ffprobe.exe"
    } else {
        "ffprobe"
    });
    command.args([
        "-hide_banner", "-loglevel", "error", "-max_alloc", "67108864", "-threads", "2",
        "-protocol_whitelist", "file", "-f", "wav", "-max_streams", "1", "-show_entries",
        "stream=codec_name,codec_type,sample_rate,channels,time_base,duration_ts,bits_per_sample:format=format_name",
        "-of", "json", "-i",
    ]).arg(stage);
    command
}

fn verify_header(
    header: &mut [u8; 44],
    length: u64,
    options: &Options<'_>,
    streamed: bool,
) -> Result<()> {
    ensure!(
        length == u64::from(options.pcm_bytes()) + 44
            && &header[..4] == b"RIFF"
            && &header[8..16] == b"WAVEfmt "
            && &header[36..40] == b"data",
        "Audio output header or sample count does not match the request"
    );
    let u16_at = |offset| u16::from_le_bytes(header[offset..offset + 2].try_into().unwrap());
    let u32_at = |offset| u32::from_le_bytes(header[offset..offset + 4].try_into().unwrap());
    ensure!(
        u32_at(16) == 16
            && u16_at(20) == 1
            && u16_at(22) == options.channels
            && u32_at(24) == RATE
            && u32_at(28) == RATE * u32::from(options.channels) * 2
            && u16_at(32) == options.channels * 2
            && u16_at(34) == 16,
        "Audio output format does not match PCM16 at 48000 Hz"
    );
    let data_length = options.pcm_bytes();
    ensure!(
        (u32_at(4) == data_length + 36 || (streamed && u32_at(4) == u32::MAX))
            && (u32_at(40) == data_length || (streamed && u32_at(40) == u32::MAX)),
        "Audio output size fields do not match the request"
    );
    header[4..8].copy_from_slice(&(data_length + 36).to_le_bytes());
    header[40..44].copy_from_slice(&data_length.to_le_bytes());
    Ok(())
}

#[derive(Deserialize)]
struct Probe {
    streams: Vec<AudioStream>,
    format: ProbeFormat,
}
#[derive(Deserialize)]
struct ProbeFormat {
    format_name: String,
}
#[derive(Deserialize)]
struct AudioStream {
    codec_name: String,
    codec_type: String,
    sample_rate: String,
    channels: u16,
    bits_per_sample: u16,
    time_base: String,
    duration_ts: u64,
}

fn verify_probe(bytes: &[u8], options: &Options<'_>) -> Result<()> {
    let result: Probe = serde_json::from_slice(bytes)
        .map_err(|_| anyhow::anyhow!("Invalid audio verification result"))?;
    ensure!(
        result.format.format_name == "wav" && result.streams.len() == 1,
        "Audio stream/container verification failed"
    );
    let stream = &result.streams[0];
    ensure!(
        stream.codec_name == "pcm_s16le"
            && stream.codec_type == "audio"
            && stream.sample_rate == "48000"
            && stream.channels == options.channels
            && stream.bits_per_sample == 16
            && stream.time_base == "1/48000"
            && stream.duration_ts == u64::from(options.samples()),
        "Audio codec, rate, channels or sample count verification failed"
    );
    Ok(())
}

pub(crate) fn run(args: &[OsString]) -> Result<()> {
    let options = options(args)?;
    #[cfg(not(any(unix, target_os = "windows")))]
    {
        let _ = options;
        bail!("Audio recording is unsupported on this platform");
    }
    #[cfg(any(unix, target_os = "windows"))]
    record(options)
}

#[cfg(any(unix, target_os = "windows"))]
fn record(options: Options<'_>) -> Result<()> {
    #[cfg(unix)]
    use crate::record::unix::{Cancellation, Staging};
    #[cfg(target_os = "windows")]
    use crate::record_windows::Staging;
    let destination = std::path::absolute(options.destination)?;
    let parent = destination
        .parent()
        .context("Audio output has no parent directory")?;
    #[cfg(unix)]
    let cancellation = Cancellation::new()?;
    let deadline = Instant::now() + Duration::from_secs(u64::from(options.seconds) + 10);
    let staging = Staging::new(parent, crate::record::OutputFormat::Matroska)?;
    let result = (|| -> Result<u64> {
        #[cfg(unix)]
        crate::record::unix::engine(
            encoder(&options, &staging.file),
            false,
            &cancellation,
            deadline,
        )?;
        #[cfg(target_os = "windows")]
        crate::record_windows::engine(
            encoder(&options, &staging.file),
            Some(&staging.file),
            deadline,
        )?;
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&staging.file)?;
        let metadata = file.metadata()?;
        ensure!(metadata.is_file(), "Audio output is not a regular file");
        let mut header = [0; 44];
        file.read_exact(&mut header)
            .context("Audio output is incomplete")?;
        verify_header(
            &mut header,
            metadata.len(),
            &options,
            cfg!(target_os = "windows"),
        )?;
        // Streaming WAVs cannot seek to complete RIFF sizes until the engine exits.
        file.seek(SeekFrom::Start(0))?;
        file.write_all(&header)?;
        file.flush()?;
        #[cfg(unix)]
        let verification =
            crate::record::unix::engine(probe(&staging.file), true, &cancellation, deadline)?;
        #[cfg(target_os = "windows")]
        let verification = crate::record_windows::engine(probe(&staging.file), None, deadline)?;
        verify_probe(&verification, &options)?;
        file.sync_all()?;
        drop(file);
        #[cfg(unix)]
        cancellation.check(deadline)?;
        ensure!(
            Instant::now() < deadline,
            "Audio exceeded its duration plus 10 second deadline"
        );
        std::fs::hard_link(&staging.file, &destination)
            .context("Could not publish audio without replacing an existing file")?;
        #[cfg(unix)]
        std::fs::File::open(parent)
            .and_then(|directory| directory.sync_all())
            .context("Audio was published, but directory durability could not be confirmed")?;
        Ok(metadata.len())
    })();
    let cleanup = staging.cleanup();
    let bytes = match (result, cleanup) {
        (Ok(bytes), Ok(())) => bytes,
        (Err(error), Ok(())) => return Err(error),
        (Ok(_), Err(error)) => {
            return Err(error.context("Audio was published, but staging cleanup failed"));
        }
        (Err(error), Err(_)) => {
            return Err(error.context("Audio failed and staging cleanup also failed"));
        }
    };
    #[cfg(unix)]
    drop(cancellation);
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "{}", serde_json::json!({
        "mode":options.mode,"container":"wav","codec":"pcm_s16le","seconds":options.seconds,
        "sample_rate":RATE,"channels":options.channels,"samples":options.samples(),"bytes":bytes,
    })).and_then(|_| stdout.flush()).context("Audio was published, but its result could not be written")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(mode: &str) -> Options<'_> {
        Options {
            mode,
            destination: Path::new("unused.wav"),
            device: Some("default"),
            seconds: 1,
            channels: 2,
        }
    }

    #[test]
    fn native_device_syntax_cannot_select_an_extra_video_or_audio_input() {
        for (mode, valid, invalid) in [
            ("pulse", "alsa_output.device.monitor", "default:video=0"),
            ("alsa", "hw:0,0", "../device"),
            ("dshow", "Microphone (USB Audio)", "Microphone:video=Camera"),
            ("avfoundation", "0", "0:1"),
        ] {
            assert!(device(mode, valid).is_ok());
            assert!(device(mode, invalid).is_err());
            for invalid in ["", " default", "default\n", &"x".repeat(257)] {
                assert!(device(mode, invalid).is_err());
            }
        }
    }

    #[test]
    fn all_encoder_inputs_use_fixed_audio_mapping_and_no_shell() {
        for mode in ["test", "pulse", "alsa", "dshow", "avfoundation"] {
            let command = encoder(&fixture(mode), Path::new("stage"));
            let args = command
                .get_args()
                .map(|arg| arg.to_str().unwrap())
                .collect::<Vec<_>>();
            assert!(args.windows(2).any(|pair| pair == ["-map", "0:a:0"]));
            assert!(args.windows(2).any(|pair| pair == ["-c:a", "pcm_s16le"]));
            assert!(args.windows(2).any(|pair| pair == ["-f", "wav"]));
            assert!(args.contains(&"-vn"));
            assert!(args.contains(&"-nostdin"));
            if mode == "avfoundation" {
                assert!(args.contains(&"none:default"));
            }
            if mode == "dshow" {
                assert!(args.contains(&"audio=default"));
            }
        }
    }

    #[test]
    fn header_verification_requires_exact_pcm_and_completes_only_streamed_sizes() {
        let options = fixture("test");
        let mut header = [0; 44];
        header[..4].copy_from_slice(b"RIFF");
        header[8..16].copy_from_slice(b"WAVEfmt ");
        header[16..20].copy_from_slice(&16u32.to_le_bytes());
        header[20..22].copy_from_slice(&1u16.to_le_bytes());
        header[22..24].copy_from_slice(&2u16.to_le_bytes());
        header[24..28].copy_from_slice(&RATE.to_le_bytes());
        header[28..32].copy_from_slice(&(RATE * 4).to_le_bytes());
        header[32..34].copy_from_slice(&4u16.to_le_bytes());
        header[34..36].copy_from_slice(&16u16.to_le_bytes());
        header[36..40].copy_from_slice(b"data");
        header[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        header[40..44].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(verify_header(&mut header, 192044, &options, false).is_err());
        verify_header(&mut header, 192044, &options, true).unwrap();
        assert_eq!(u32::from_le_bytes(header[4..8].try_into().unwrap()), 192036);
        assert_eq!(
            u32::from_le_bytes(header[40..44].try_into().unwrap()),
            192000
        );
        assert!(verify_header(&mut header, 192045, &options, true).is_err());
        header[22] = 1;
        assert!(verify_header(&mut header, 192044, &options, true).is_err());
    }

    #[test]
    fn probe_rejects_incomplete_other_codec_or_extra_stream_results() {
        let options = fixture("test");
        let stream = serde_json::json!({"codec_name":"pcm_s16le","codec_type":"audio","sample_rate":"48000","channels":2,"bits_per_sample":16,"time_base":"1/48000","duration_ts":48000});
        let valid = serde_json::json!({"streams":[stream],"format":{"format_name":"wav"}});
        verify_probe(&serde_json::to_vec(&valid).unwrap(), &options).unwrap();
        for field in ["codec_name", "codec_type", "sample_rate", "time_base"] {
            let mut invalid = valid.clone();
            invalid["streams"][0][field] = "invalid".into();
            assert!(verify_probe(&serde_json::to_vec(&invalid).unwrap(), &options).is_err());
        }
        for field in ["channels", "bits_per_sample", "duration_ts"] {
            let mut invalid = valid.clone();
            invalid["streams"][0][field] = 1.into();
            assert!(verify_probe(&serde_json::to_vec(&invalid).unwrap(), &options).is_err());
        }
        let invalid = serde_json::json!({"streams":[stream,stream],"format":{"format_name":"wav"}});
        assert!(verify_probe(&serde_json::to_vec(&invalid).unwrap(), &options).is_err());
    }
}
