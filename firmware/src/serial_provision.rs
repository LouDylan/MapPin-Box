//! Local provisioning over the board's USB console UART.

use std::{
    thread,
    time::{Duration, Instant},
};

use enigma_embedded_bridge_protocol::{Header, MAX_PAIRING_RECORD_BYTES, verify_payload};
use esp_idf_svc::{
    hal::{delay::TickType, uart::UartDriver},
    sys,
};

use crate::app::{App, PairingBusy};
use std::sync::Arc;

const HEADER_LIMIT: usize = 160;
const READ_SLICE: Duration = Duration::from_millis(250);
const PAYLOAD_TIMEOUT: Duration = Duration::from_secs(15);
const HEADER_POLL_WINDOW: Duration = Duration::from_secs(60);
const LISTENER_STACK_BYTES: usize = 12 * 1024;
const FRAME_PREFIX: &str = "MAPPIN-PROVISION/1";

enum Kind {
    Pairing,
    Amap,
}

impl Kind {
    fn name(&self) -> &'static str {
        match self {
            Self::Pairing => "pairing",
            Self::Amap => "amap",
        }
    }

    fn limit(&self) -> usize {
        match self {
            Self::Pairing => MAX_PAIRING_RECORD_BYTES,
            Self::Amap => 1024,
        }
    }

    fn apply(&self, app: &App, payload: &[u8]) -> Result<&'static str, &'static str> {
        match self {
            Self::Pairing => app
                .import_pairing(payload)
                .map(|()| "saved")
                .map_err(|error| {
                    if error.is::<PairingBusy>() {
                        "busy"
                    } else {
                        "rejected"
                    }
                }),
            Self::Amap => {
                let credentials = serde_json::from_slice(payload).map_err(|_| "invalid")?;
                app.seed_amap(credentials)
                    .map(|written| if written { "written" } else { "kept" })
                    .map_err(|_| "rejected")
            }
        }
    }
}

struct Frame {
    kind: Kind,
    header: Header,
}

pub fn start(uart: UartDriver<'static>, app: Arc<App>) -> Result<(), String> {
    thread::Builder::new()
        .name("mappin-provision".into())
        .stack_size(LISTENER_STACK_BYTES)
        .spawn(move || run_listener(uart, app))
        .map(|_| ())
        .map_err(|error| format!("could not start pairing listener: {error}"))
}

fn run_listener(uart: UartDriver<'static>, app: Arc<App>) {
    log::info!("MAPPIN_PROVISION_READY");
    loop {
        match receive_frame(&uart) {
            Ok(Some((frame, payload))) => {
                let result = verify_payload(&frame.header, &payload)
                    .map_err(|_| "checksum")
                    .and_then(|()| frame.kind.apply(&app, &payload));
                let (code, status) = match result {
                    Ok(status) => ("OK", status),
                    Err(status) => ("ERROR", status),
                };
                let reply = format!(
                    "MAPPIN_PROVISION_{code} {} {} {status}\n",
                    frame.kind.name(),
                    frame.header.sha256
                );
                if uart.write(reply.as_bytes()).is_err() {
                    log::warn!("MAPPIN_PROVISION_REPLY_FAILED");
                }
            }
            Ok(None) => {}
            Err(error) => log::warn!("MAPPIN_PROVISION_FRAME_ERROR: {error}"),
        }
    }
}

fn receive_frame(uart: &UartDriver<'_>) -> Result<Option<(Frame, Vec<u8>)>, String> {
    let Some((frame, payload_prefix)) = read_header(uart, HEADER_POLL_WINDOW)? else {
        return Ok(None);
    };
    if payload_prefix.len() > frame.header.payload_len {
        return Err("payload started with more bytes than declared".into());
    }
    let payload = read_exact_with_timeout(
        uart,
        frame.header.payload_len,
        &payload_prefix,
        PAYLOAD_TIMEOUT,
    )?;
    Ok(Some((frame, payload)))
}

fn parse_line(uart: &UartDriver<'_>, line: &[u8]) -> Result<Option<Frame>, String> {
    let Ok(text) = core::str::from_utf8(line) else {
        return Ok(None);
    };
    if text.trim() == "MAPPIN-INFO/1" {
        let reply = format!(
            "MAPPIN_INFO/1 version={} protocol=1\n",
            env!("CARGO_PKG_VERSION")
        );
        uart.write(reply.as_bytes())
            .map_err(|error| error.to_string())?;
        return Ok(None);
    }
    let Some(rest) = text.trim().strip_prefix(FRAME_PREFIX) else {
        return Ok(None);
    };
    let mut fields = rest.split_ascii_whitespace();
    let kind = match fields.next() {
        Some("pairing") => Kind::Pairing,
        Some("amap") => Kind::Amap,
        _ => return Err("unknown provisioning kind".into()),
    };
    let payload_len = fields
        .next()
        .and_then(|field| field.parse::<usize>().ok())
        .filter(|size| (1..=kind.limit()).contains(size))
        .ok_or("invalid provisioning length")?;
    let sha256 = fields
        .next()
        .filter(|value| value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .ok_or("invalid provisioning checksum")?;
    if fields.next().is_some() {
        return Err("unexpected provisioning header field".into());
    }
    Ok(Some(Frame {
        kind,
        header: Header {
            payload_len,
            sha256: sha256.to_ascii_lowercase(),
        },
    }))
}

fn read_header(
    uart: &UartDriver<'_>,
    timeout: Duration,
) -> Result<Option<(Frame, Vec<u8>)>, String> {
    let deadline = Instant::now() + timeout;
    let mut line = Vec::with_capacity(HEADER_LIMIT);
    let mut chunk = [0u8; 128];
    while Instant::now() < deadline {
        let count = match uart.read(
            &mut chunk,
            TickType::new_millis(READ_SLICE.as_millis() as u64).ticks(),
        ) {
            Ok(count) => count,
            Err(error) if error.code() == sys::ESP_ERR_TIMEOUT => continue,
            Err(error) => return Err(format!("UART provisioning read failed: {error}")),
        };
        for (index, byte) in chunk[..count].iter().enumerate() {
            if *byte != b'\n' {
                if line.len() < HEADER_LIMIT {
                    line.push(*byte);
                } else {
                    line.clear();
                }
                continue;
            }
            let header = parse_line(uart, &line)?;
            line.clear();
            let Some(header) = header else {
                continue;
            };
            // A read may contain both the header terminator and initial payload bytes.
            return Ok(Some((header, chunk[index + 1..count].to_vec())));
        }
    }
    Ok(None)
}

fn read_exact_with_timeout(
    uart: &UartDriver<'_>,
    length: usize,
    prefix: &[u8],
    timeout: Duration,
) -> Result<Vec<u8>, String> {
    let deadline = Instant::now() + timeout;
    let mut payload = vec![0; length];
    payload[..prefix.len()].copy_from_slice(prefix);
    let mut offset = prefix.len();
    while offset < length && Instant::now() < deadline {
        match uart.read(
            &mut payload[offset..],
            TickType::new_millis(READ_SLICE.as_millis() as u64).ticks(),
        ) {
            Ok(count) => offset += count,
            Err(error) if error.code() == sys::ESP_ERR_TIMEOUT => {}
            Err(error) => return Err(format!("UART provisioning payload read failed: {error}")),
        }
    }
    if offset == length {
        Ok(payload)
    } else {
        Err(format!(
            "provisioning payload timed out after {offset}/{length} bytes"
        ))
    }
}
