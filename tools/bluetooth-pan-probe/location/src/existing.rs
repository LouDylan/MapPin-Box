//! Load an existing remote identity for this boot through the physical UART.

use crate::{
    IphoneController, Result, RpPairingFile, SESSION, iphone_session, pairing, sdk,
    mappin_pan_gateway, mappin_setup_command,
};
use std::{
    ffi::{CStr, c_char},
    net::IpAddr,
    time::{Duration, Instant},
};

const MAX_RECORD: usize = 2048;

pub unsafe extern "C" fn command(line: *const c_char) -> bool {
    let raw = unsafe { CStr::from_ptr(line) }.to_bytes();
    let Some(length) = raw.strip_prefix(b"use-existing ") else {
        return unsafe { mappin_setup_command(line) };
    };
    if let Err(error) = start(length) {
        log::warn!("MAPPIN_BT_EXISTING_FAILED: {error}");
    }
    true
}

fn start(length: &[u8]) -> Result<()> {
    let length: usize = std::str::from_utf8(length)?.parse()?;
    if !(1..=MAX_RECORD).contains(&length) {
        return Err("invalid remote record length".into());
    }
    let gateway = unsafe { mappin_pan_gateway() };
    if gateway == 0 || pairing::busy()? {
        return Err("Bluetooth PAN must be connected and idle".into());
    }
    log::info!("MAPPIN_BT_EXISTING_INPUT");
    let payload = read_record(length)?;
    let peer = IpAddr::from(gateway.to_ne_bytes());
    iphone_session::spawn_worker(move || match authenticate(&payload, peer) {
        Ok(()) => log::info!("MAPPIN_BT_EXISTING_READY"),
        Err(error) => log::warn!("MAPPIN_BT_EXISTING_FAILED: {error}"),
    })
}

fn read_record(length: usize) -> Result<Vec<u8>> {
    let mut bytes = vec![0; length];
    let mut filled = 0;
    let deadline = Instant::now() + Duration::from_secs(5);
    while filled < length && Instant::now() < deadline {
        let chunk = &mut bytes[filled..];
        let count = unsafe {
            sdk::uart_read_bytes(
                sdk::uart_port_t_UART_NUM_0,
                chunk.as_mut_ptr().cast(),
                chunk.len() as _,
                10,
            )
        };
        if count < 0 {
            return Err("cannot read remote record".into());
        }
        filled += count as usize;
    }
    if filled != length {
        return Err("remote record input timed out".into());
    }
    Ok(bytes)
}

fn authenticate(bytes: &[u8], peer: IpAddr) -> Result<()> {
    let record = RpPairingFile::from_bytes(bytes).map_err(|_| "invalid remote record")?;
    let session = SESSION.get().ok_or("location worker unavailable")?;
    let mut state = session.lock().map_err(|_| "location worker unavailable")?;
    if state.restore_at.is_some() {
        return Err("restore real location first".into());
    }
    let controller = IphoneController::connect_remote(record.clone(), peer, None)?;
    controller.check()?;
    state.temporary_pairing = Some(record);
    state.peer = Some(peer);
    state.controller = Some(controller);
    Ok(())
}
