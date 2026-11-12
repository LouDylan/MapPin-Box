//! Developer pairing and location commands over Classic Bluetooth PAN.

mod existing;
#[path = "../../../../firmware/src/iphone_session.rs"]
mod iphone_session;
mod pairing;

use enigma_embedded_bridge_protocol::{MAX_PAIRING_RECORD_BYTES, decode_pairing_bundle};
use enigma_embedded_core::Location;
use esp_idf_sys as sdk;
use idevice::remote_pairing::RpPairingFile;
use iphone_session::IphoneController;
use sdk::EspError;
use std::{
    error::Error,
    ffi::{CStr, c_char},
    net::{IpAddr, Ipv4Addr},
    ptr,
    sync::{Mutex, OnceLock},
    thread,
    time::{Duration, Instant},
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;
const TEST_WINDOW: Duration = Duration::from_secs(180);
const PAIR_RECORD: &CStr = c"pan_pair_v1";
static SESSION: OnceLock<Mutex<Session>> = OnceLock::new();

unsafe extern "C" {
    fn mappin_pan_start(diagnostic: bool, command: unsafe extern "C" fn(*const c_char) -> bool);
    fn mappin_pan_gateway() -> u32;
    fn mappin_pan_bond_count() -> i32;
    fn mappin_setup_start() -> sdk::esp_err_t;
    fn mappin_setup_stop();
    fn mappin_setup_address() -> u32;
    fn mappin_setup_sta_address() -> u32;
    fn mappin_setup_command(command: *const c_char) -> bool;
    fn mappin_setup_ifindex(address: u32) -> i32;
    fn mappin_pair_trace(port: u16);
    fn mappin_location_mdns_enable(address: u32) -> sdk::esp_err_t;
    fn mappin_location_mdns_report(address: u32);
    fn mappin_location_http_start(html: *const u8, size: usize) -> sdk::esp_err_t;
}

struct RecordStore(sdk::nvs_handle_t);

impl RecordStore {
    fn open() -> Result<Self> {
        let partition = c"userdata";
        EspError::convert(unsafe { sdk::nvs_flash_init_partition(partition.as_ptr()) })?;
        let mut handle = 0;
        EspError::convert(unsafe {
            sdk::nvs_open_from_partition(
                partition.as_ptr(),
                c"idevice".as_ptr(),
                sdk::nvs_open_mode_t_NVS_READWRITE,
                &mut handle,
            )
        })?;
        Ok(Self(handle))
    }

    fn read(&self, key: &CStr) -> Result<Option<Vec<u8>>> {
        let mut len = 0;
        let result = unsafe { sdk::nvs_get_blob(self.0, key.as_ptr(), ptr::null_mut(), &mut len) };
        if result == sdk::ESP_ERR_NVS_NOT_FOUND {
            return Ok(None);
        }
        EspError::convert(result)?;
        if len > MAX_PAIRING_RECORD_BYTES {
            return Err("配对资料超过大小限制".into());
        }
        let mut bytes = vec![0; len];
        EspError::convert(unsafe {
            sdk::nvs_get_blob(self.0, key.as_ptr(), bytes.as_mut_ptr().cast(), &mut len)
        })?;
        bytes.truncate(len);
        Ok(Some(bytes))
    }

    fn save_pairing(&self, record: &RpPairingFile) -> Result<()> {
        let bytes = record.to_bytes();
        EspError::convert(unsafe {
            sdk::nvs_set_blob(
                self.0,
                PAIR_RECORD.as_ptr(),
                bytes.as_ptr().cast(),
                bytes.len(),
            )
        })?;
        EspError::convert(unsafe { sdk::nvs_commit(self.0) })?;
        Ok(())
    }

    fn load_pairing(&self) -> Result<RpPairingFile> {
        if let Some(bytes) = self.read(PAIR_RECORD)? {
            return Ok(RpPairingFile::from_bytes(&bytes)?);
        }
        if let Some(bytes) = self.read(c"pair_bundle_v2")? {
            let bundle = decode_pairing_bundle(&bytes).map_err(|err| err.to_string())?;
            let remote = bundle.remote.ok_or("配对备份缺少开发者配对资料")?;
            return Ok(RpPairingFile::from_bytes(remote)?);
        }
        Err("请先配对定位服务".into())
    }
}

struct Session {
    pairing: RecordStore,
    temporary_pairing: Option<RpPairingFile>,
    controller: Option<IphoneController>,
    peer: Option<IpAddr>,
    restore_at: Option<Instant>,
}

impl Session {
    fn command(&mut self, peer: IpAddr, set: bool) -> Result<()> {
        if self.peer != Some(peer) {
            self.controller = None;
            self.peer = Some(peer);
        }
        if self.controller.is_none() {
            let record = match &self.temporary_pairing {
                Some(record) => record.clone(),
                None => self.pairing.load_pairing()?,
            };
            self.controller = Some(IphoneController::connect_remote(record, peer, None)?);
        }
        let controller = self.controller.as_mut().expect("controller initialized");
        let result = if set {
            self.restore_at = Some(Instant::now() + TEST_WINDOW);
            controller.set(&Location::new("上海市中心", "31.2304", "121.4737"))
        } else {
            controller.restore()
        };
        if result.is_err() {
            self.controller = None;
        } else if !set {
            self.restore_at = None;
        }
        result
    }

    fn restore_if_due(&mut self) {
        if let Some(deadline) = self.restore_at
            && let Some(peer) = self.peer
            && Instant::now() >= deadline
            && let Err(error) = self.command(peer, false)
        {
            log::warn!("Automatic Bluetooth restore failed: {error}");
            self.restore_at = Some(Instant::now() + Duration::from_secs(15));
        }
    }
}

fn dispatch(uri: &CStr, local: Ipv4Addr, peer: IpAddr) -> Result<(i32, String)> {
    if uri == c"/pair-status" {
        return Ok((200, pairing::message(local, peer)?));
    }
    if uri == c"/pair" {
        pairing::start(local, peer)?;
        return Ok((202, pairing::message(local, peer)?));
    }
    let set = uri == c"/set";
    if set && pairing::busy()? {
        return Err("请先完成开发者配对".into());
    }
    let session = SESSION.get().ok_or("定位会话尚未启动")?;
    session
        .lock()
        .map_err(|_| "定位会话不可用")?
        .command(peer, set)?;
    let message = if set {
        "已设置上海市中心。请在苹果地图确认。"
    } else {
        "已恢复真实定位。请在苹果地图确认。"
    };
    Ok((200, message.into()))
}

/// The C server supplies a valid URI and a writable response buffer.
#[unsafe(no_mangle)]
unsafe extern "C" fn mappin_location_command(
    uri: *const c_char,
    local: u32,
    peer: u32,
    reply: *mut u8,
    capacity: usize,
) -> i32 {
    let address = IpAddr::from(peer.to_ne_bytes());
    let local = Ipv4Addr::from(local.to_ne_bytes());
    let (status, text) =
        dispatch(unsafe { CStr::from_ptr(uri) }, local, address).unwrap_or_else(|error| {
            log::warn!("MAPPIN_BT_COMMAND_FAILED: {error}");
            (502, error.to_string())
        });
    let mut len = text.len().min(capacity - 1);
    while !text.is_char_boundary(len) {
        len -= 1;
    }
    unsafe {
        ptr::copy_nonoverlapping(text.as_ptr(), reply, len);
        reply.add(len).write(0);
    }
    status
}

struct Logger;

impl log::Log for Logger {
    fn enabled(&self, _: &log::Metadata<'_>) -> bool {
        true
    }
    fn log(&self, record: &log::Record<'_>) {
        println!("{} {}", record.level(), record.args());
    }
    fn flush(&self) {}
}

fn run() -> Result<()> {
    sdk::link_patches();
    log::set_logger(&Logger).map_err(|error| error.to_string())?;
    log::set_max_level(log::LevelFilter::Info);
    let config = sdk::esp_vfs_eventfd_config_t { max_fds: 5 };
    EspError::convert(unsafe { sdk::esp_vfs_eventfd_register(&config) })?;
    let pairing = RecordStore::open()?;
    let needs_setup = pairing.load_pairing().is_err();
    pairing::set_saved(!needs_setup);
    if !needs_setup {
        log::info!("MAPPIN_FIRST_SETUP_SAVED");
    }
    let session = Session {
        pairing,
        temporary_pairing: None,
        controller: None,
        peer: None,
        restore_at: None,
    };
    SESSION
        .set(Mutex::new(session))
        .map_err(|_| "定位会话已经启动")?;
    let session = SESSION.get().ok_or("定位会话尚未启动")?;
    unsafe { mappin_pan_start(false, existing::command) };
    let page = include_bytes!("index.html");
    EspError::convert(unsafe { mappin_location_http_start(page.as_ptr(), page.len()) })?;
    if needs_setup {
        log::info!("MAPPIN_FIRST_SETUP_REQUIRED");
        if unsafe { mappin_pan_bond_count() } == 0 {
            if let Err(error) = EspError::convert(unsafe { mappin_setup_start() }) {
                log::warn!("MAPPIN_FIRST_SETUP_START_FAILED: {error}");
            }
        }
    }
    log::info!("MAPPIN_BT_LOCATION_READY");
    loop {
        if let Ok(mut session) = session.try_lock() {
            pairing::commit_pending(&mut session);
            session.restore_if_due();
        }
        thread::sleep(Duration::from_millis(250));
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("MAPPIN_BT_LOCATION_STARTUP_ERROR: {error}");
    }
}
