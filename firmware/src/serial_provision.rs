//! Always-available pairing-record import over the board's CH340K console UART.

use std::{
    thread,
    time::{Duration, Instant},
};

use enigma_embedded_bridge_protocol::{
    ERROR_PREFIX, HEADER_PREFIX, READY_LINE, parse_header, success_line, verify_payload,
};
use esp_idf_svc::{
    hal::{delay::TickType, uart::UartDriver},
    sys,
};

use crate::app::App;
use std::sync::Arc;

const HEADER_LIMIT: usize = 160;
const READ_SLICE: Duration = Duration::from_millis(250);
const PAYLOAD_TIMEOUT: Duration = Duration::from_secs(15);
const HEADER_POLL_WINDOW: Duration = Duration::from_secs(60);
const LISTENER_STACK_BYTES: usize = 12 * 1024;

pub fn start(uart: UartDriver<'static>, app: Arc<App>) -> Result<(), String> {
    thread::Builder::new()
        .name("waypin-provision".into())
        .stack_size(LISTENER_STACK_BYTES)
        .spawn(move || run_listener(uart, app))
        .map(|_| ())
        .map_err(|error| format!("could not start pairing listener: {error}"))
}

fn run_listener(uart: UartDriver<'static>, app: Arc<App>) {
    log::info!("{READY_LINE}");
    log::info!("persistent desktop pairing listener ready");
    loop {
        match receive_pairing_record(&uart) {
            Ok(Some(payload)) => match app.import_pairing(&payload) {
                Ok(()) => log::info!("{}", success_line(&payload)),
                Err(error) => log::error!("{ERROR_PREFIX} invalid pairing record: {error}"),
            },
            Ok(None) => {}
            Err(error) => log::error!("{ERROR_PREFIX} {error}"),
        }
    }
}

fn receive_pairing_record(uart: &UartDriver<'_>) -> Result<Option<Vec<u8>>, String> {
    let Some((header, payload_prefix)) = read_header(uart, HEADER_POLL_WINDOW)? else {
        return Ok(None);
    };
    if payload_prefix.len() > header.payload_len {
        return Err("pairing payload started with more bytes than declared".into());
    }
    let payload =
        read_exact_with_timeout(uart, header.payload_len, &payload_prefix, PAYLOAD_TIMEOUT)?;
    verify_payload(&header, &payload).map_err(|error| error.to_string())?;
    Ok(Some(payload))
}

fn parse_line(
    uart: &UartDriver<'_>,
    line: &[u8],
) -> Result<Option<enigma_embedded_bridge_protocol::Header>, String> {
    let Ok(text) = core::str::from_utf8(line) else {
        return Ok(None);
    };
    // Keep the serial handshake compatible with existing setup scripts.
    if text.trim() == "I4-INFO/1" {
        let reply = format!(
            "I4_INFO/1 version={} protocol=2\n",
            env!("CARGO_PKG_VERSION")
        );
        uart.write(reply.as_bytes())
            .map_err(|error| error.to_string())?;
        return Ok(None);
    }
    let Some(offset) = text.find(HEADER_PREFIX) else {
        return Ok(None);
    };
    parse_header(&text[offset..])
        .map(Some)
        .map_err(|error| error.to_string())
}

fn read_header(
    uart: &UartDriver<'_>,
    timeout: Duration,
) -> Result<Option<(enigma_embedded_bridge_protocol::Header, Vec<u8>)>, String> {
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
            Err(error) => return Err(format!("UART pairing payload read failed: {error}")),
        }
    }
    if offset == length {
        Ok(payload)
    } else {
        Err(format!(
            "pairing payload timed out after {offset}/{length} bytes"
        ))
    }
}
