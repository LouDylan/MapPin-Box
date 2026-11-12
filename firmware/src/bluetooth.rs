use mappin_box_model::indicator::Pattern;
use std::net::Ipv4Addr;

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Endpoint {
    pub local: Ipv4Addr,
    pub peer: Ipv4Addr,
}

#[cfg(not(esp32))]
pub fn endpoint() -> Option<Endpoint> {
    None
}

#[cfg(not(esp32))]
pub fn button_pattern() -> Option<Pattern> {
    None
}

#[cfg(esp32)]
mod classic {
    use super::{Endpoint, Ipv4Addr, Pattern};
    use crate::sdk::{self, EspError};
    use serde::Deserialize;
    use serde_json::{Value, json};
    use std::{
        ffi::{CStr, c_char},
        net::IpAddr,
        sync::atomic::{AtomicBool, Ordering},
    };

    static STARTED: AtomicBool = AtomicBool::new(false);

    unsafe extern "C" {
        fn mappin_bt_start() -> sdk::esp_err_t;
        fn mappin_bt_endpoint(local: *mut u32, peer: *mut u32) -> bool;
        fn mappin_bt_state(code: *mut u32, button: *mut bool) -> u32;
        fn mappin_bt_candidate(index: u32, name: *mut c_char) -> bool;
        fn mappin_bt_action(action: u32, value: u32) -> sdk::esp_err_t;
        fn mappin_bt_button() -> bool;
    }

    pub fn start() {
        match EspError::convert(unsafe { mappin_bt_start() }) {
            Ok(()) => STARTED.store(true, Ordering::Release),
            Err(error) => log::warn!("MAPPIN_BT_UNAVAILABLE: {error}; Wi-Fi remains available"),
        }
    }

    pub fn endpoint() -> Option<Endpoint> {
        if !STARTED.load(Ordering::Acquire) {
            return None;
        }
        let (mut local, mut peer) = (0, 0);
        unsafe { mappin_bt_endpoint(&mut local, &mut peer) }.then(|| Endpoint {
            local: Ipv4Addr::from(local.to_ne_bytes()),
            peer: Ipv4Addr::from(peer.to_ne_bytes()),
        })
    }

    pub fn peer() -> Option<IpAddr> {
        endpoint().map(|endpoint| endpoint.peer.into())
    }

    pub fn button() -> bool {
        STARTED.load(Ordering::Acquire) && unsafe { mappin_bt_button() }
    }

    pub fn button_pattern() -> Option<Pattern> {
        let (mut code, mut button) = (0, false);
        unsafe { mappin_bt_state(&mut code, &mut button) };
        button.then_some(Pattern::BluetoothPairing)
    }

    #[derive(Deserialize)]
    #[serde(tag = "action", rename_all = "snake_case")]
    pub enum Action {
        Scan,
        Connect { device: u32 },
        Confirm { code: u32 },
    }

    impl Action {
        pub fn submit(self) -> Result<(), &'static str> {
            if !STARTED.load(Ordering::Acquire) {
                return Err("蓝牙不可用，请使用备用 Wi-Fi");
            }
            let (action, value) = match self {
                Self::Scan => (0, 0),
                Self::Connect { device } => (1, device),
                Self::Confirm { code } => (2, code),
            };
            EspError::convert(unsafe { mappin_bt_action(action, value) })
                .map_err(|_| "蓝牙状态已变化，请刷新后重试")
        }
    }

    pub fn snapshot() -> Value {
        if !STARTED.load(Ordering::Acquire) {
            return json!({"state": "unavailable"});
        }
        let (mut code, mut button) = (0, false);
        let phase = unsafe { mappin_bt_state(&mut code, &mut button) };
        let address = endpoint().map(|endpoint| endpoint.local);
        let state = match (phase, button) {
            (_, true) => "button_pairing",
            (0, false) => "starting",
            (1, false) => "idle",
            (2, false) => "scanning",
            (3, false) => "connecting",
            (4, false) => "confirming",
            (5, false) if address.is_some() => "connected",
            (5, false) => "connecting",
            _ => "failed",
        };
        let mut devices = Vec::new();
        if matches!(state, "idle" | "failed") {
            let mut name = [0; 65];
            for id in 0.. {
                if !unsafe { mappin_bt_candidate(id, name.as_mut_ptr()) } {
                    break;
                }
                let name = unsafe { CStr::from_ptr(name.as_ptr()) }.to_string_lossy();
                let name: String = name.chars().filter(|c| !c.is_control()).collect();
                devices.push(json!({"id": id, "name": name}));
            }
        }
        json!({"state": state, "address": address, "devices": devices,
               "code": (phase == 4 && !button).then(|| format!("{code:06}"))})
    }
}

#[cfg(esp32)]
pub use classic::*;
