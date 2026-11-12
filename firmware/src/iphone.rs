use std::{
    error::Error,
    sync::atomic::{AtomicU32, Ordering},
};

use enigma_embedded_bridge_protocol::{MAX_PAIRING_RECORD_BYTES, decode_pairing_bundle};
use esp_idf_svc::{
    nvs::{EspKeyValueStorage, EspNvs, NvsCustom},
    sys::{self, EspError},
};
use idevice::{pairing_file::PairingFile, remote_pairing::RpPairingFile};
use plist::Data;
use serde::{Deserialize, Serialize};

pub use crate::iphone_session::IphoneController;
use crate::iphone_session::stage_error;
use crate::persistent_storage::PersistentNvsPartition;

const BUNDLE_KEY: &str = "pair_bundle_v2";
const PAN_KEY: &str = "pan_pair_v1";
const ACTIVE_KEY: &str = "pairing_v3";
const STORED_LIMIT: usize = MAX_PAIRING_RECORD_BYTES + 1024;
static PAIRING_GENERATION: AtomicU32 = AtomicU32::new(0);

#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct PhoneInfo {
    pub name: String,
    pub product_type: String,
    pub product_version: String,
}

pub fn pairing_generation() -> u32 {
    PAIRING_GENERATION.load(Ordering::Acquire)
}

const REMOTE_PAIRING_KEY: &str = "rp_record";
pub type PairingStorage = EspKeyValueStorage<NvsCustom>;

pub fn pairing_storage(
    partition: PersistentNvsPartition,
) -> Result<PairingStorage, Box<dyn Error>> {
    let nvs = EspNvs::new(partition, "idevice", true)?;
    Ok(EspKeyValueStorage::new(nvs))
}

/// Both setup paths replace the same active record in one NVS commit.
#[derive(Deserialize, Serialize)]
pub struct SavedPairing {
    remote: Data,
    lockdown: Option<Data>,
    pub phone: Option<PhoneInfo>,
}

impl SavedPairing {
    pub fn serial(bytes: &[u8]) -> Result<Self, Box<dyn Error>> {
        let mut pairing: Self = plist::from_bytes(bytes)?;
        let lockdown = pairing.lockdown.as_ref().ok_or("缺少 USB 信任资料")?;
        PairingFile::from_bytes(lockdown.as_ref())?;
        pairing.remote()?;
        pairing.phone = pairing.phone.filter(valid_profile);
        Ok(pairing)
    }

    fn bundle(bytes: &[u8]) -> Result<Self, Box<dyn Error>> {
        let bundle = decode_pairing_bundle(bytes).map_err(|error| error.to_string())?;
        let remote = bundle.remote.ok_or("配对资料缺少开发者身份")?;
        Ok(Self {
            remote: Data::new(remote.to_vec()),
            lockdown: Some(Data::new(bundle.lockdown.to_vec())),
            phone: profile_from_lockdown(bundle.lockdown),
        })
    }

    #[cfg(esp32)]
    pub fn native(record: &RpPairingFile, phone: PhoneInfo) -> Self {
        Self {
            remote: Data::new(record.to_bytes()),
            lockdown: None,
            phone: Some(phone),
        }
    }

    fn remote(&self) -> Result<RpPairingFile, Box<dyn Error>> {
        Ok(RpPairingFile::from_bytes(self.remote.as_ref())?)
    }

    pub fn save(&self, storage: &PairingStorage) -> Result<(), Box<dyn Error>> {
        let mut bytes = Vec::new();
        plist::to_writer_binary(&mut bytes, self)?;
        if bytes.len() > STORED_LIMIT {
            return Err("配对资料超过盒子存储限制".into());
        }
        storage.set_raw(ACTIVE_KEY, &bytes)?;
        PAIRING_GENERATION.fetch_add(1, Ordering::Release);
        log::info!("iPhone pairing identity committed to flash");
        Ok(())
    }

    pub fn load(storage: &PairingStorage) -> Result<Option<Self>, Box<dyn Error>> {
        let mut buffer = vec![0; STORED_LIMIT];
        if let Some(bytes) = storage.get_raw(ACTIVE_KEY, &mut buffer)? {
            return Ok(Some(plist::from_bytes(bytes)?));
        }
        // The PAN test's record takes precedence over earlier Mac imports.
        for key in [PAN_KEY, BUNDLE_KEY, REMOTE_PAIRING_KEY] {
            let Some(bytes) = storage.get_raw(key, &mut buffer)? else {
                continue;
            };
            return if key == BUNDLE_KEY {
                Self::bundle(bytes).map(Some)
            } else {
                Ok(Some(Self {
                    remote: Data::new(bytes.to_vec()),
                    lockdown: None,
                    phone: None,
                }))
            };
        }
        Ok(None)
    }

    pub fn inspect(storage: &PairingStorage) -> Result<(bool, Option<PhoneInfo>), Box<dyn Error>> {
        let Some(pairing) = Self::load(storage)? else {
            return Ok((false, None));
        };
        pairing.remote()?;
        Ok((true, pairing.phone))
    }
}

fn profile_from_lockdown(bytes: &[u8]) -> Option<PhoneInfo> {
    #[derive(serde::Deserialize)]
    struct Record {
        #[serde(rename = "_I4Device")]
        info: Option<PhoneInfo>,
    }
    plist::from_bytes::<Record>(bytes)
        .ok()?
        .info
        .filter(valid_profile)
}

fn valid_profile(info: &PhoneInfo) -> bool {
    [&info.name, &info.product_type, &info.product_version]
        .iter()
        .all(|value| value.chars().count() <= 64 && !value.chars().any(char::is_control))
}

impl IphoneController {
    pub fn connect(
        storage: &PairingStorage,
        address: std::net::IpAddr,
    ) -> Result<Self, Box<dyn Error>> {
        let pairing = SavedPairing::load(storage)?.ok_or("iPhone developer pairing is missing")?;
        Self::connect_remote(
            pairing.remote()?,
            address,
            Some(discover_remote_pairing_port),
        )
    }
}

fn discover_remote_pairing_port(address: std::net::IpAddr) -> Result<u16, String> {
    let mut results = core::ptr::null_mut();
    let result = unsafe {
        sys::mdns_query_ptr(
            c"_remotepairing".as_ptr(),
            c"_tcp".as_ptr(),
            5_000,
            4,
            &mut results,
        )
    };
    if let Some(error) = EspError::from(result) {
        return Err(stage_error("discover iPhone remote pairing service", error));
    }

    // Select the target phone's SRV port from ESP-IDF's linked list. The high-level
    // esp-idf-svc conversion assumes default interface-description strings and
    // panics for valid custom AP descriptions, which would reboot the board.
    let mut current = results;
    let mut port = None;
    while !current.is_null() {
        let candidate = unsafe { (*current).port };
        if candidate != 0 && mdns_has_address(unsafe { (*current).addr }, address) {
            port = Some(candidate);
            break;
        }
        current = unsafe { (*current).next };
    }
    unsafe { sys::mdns_query_results_free(results) };
    port.ok_or_else(|| "iPhone remote pairing service not found; keep phone unlocked".to_string())
}

fn mdns_has_address(mut entry: *mut sys::mdns_ip_addr_t, target: std::net::IpAddr) -> bool {
    while !entry.is_null() {
        let address = unsafe { (*entry).addr };
        if address.type_ == 0 {
            let octets = unsafe { address.u_addr.ip4.addr }.to_ne_bytes();
            if std::net::IpAddr::from(octets) == target.to_canonical() {
                return true;
            }
        }
        entry = unsafe { (*entry).next };
    }
    false
}
