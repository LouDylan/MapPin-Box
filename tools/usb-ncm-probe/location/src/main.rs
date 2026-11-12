//! USB-only acceptance test using the box's existing RemotePairing record.

#[allow(dead_code)]
#[path = "../../../../firmware/src/iphone.rs"]
mod iphone;
#[path = "../../../../firmware/src/iphone_session.rs"]
mod iphone_session;

use esp_idf_svc::sys as sdk;
#[allow(dead_code)]
#[path = "../../../../vendor/enigma/apps/embedded/platforms/esp-idf/src/persistent_storage.rs"]
mod persistent_storage;
#[path = "../../../../firmware/src/status_led.rs"]
mod status_led;

use enigma_embedded_core::Location;
use esp_idf_svc::{
    hal::peripherals::Peripherals,
    handle::RawHandle,
    http::{
        Method,
        server::{Configuration, EspHttpConnection, EspHttpServer, Request},
    },
    io::{Write, vfs::MountedEventfs},
    nvs::{EspCustomNvsPartition, EspKeyValueStorage, EspNvs},
    sys::{self, EspError},
};
use mappin_box_model::indicator::Pattern;
use status_led::StatusLed;
use std::{
    error::Error,
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;
const TEST_WINDOW: Duration = Duration::from_secs(180);

unsafe extern "C" {
    fn mappin_usb_network_start();
}

struct Session {
    pairing: iphone::PairingStorage,
    controller: Option<iphone::IphoneController>,
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
            self.controller = Some(iphone::IphoneController::connect(&self.pairing, peer)?);
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
            log::warn!("Automatic USB restore failed: {error}");
            self.restore_at = Some(Instant::now() + Duration::from_secs(15));
        }
    }
}

fn usb_peer(request: &mut Request<&mut EspHttpConnection<'_>>) -> Result<IpAddr> {
    let socket = unsafe { sys::httpd_req_to_sockfd(request.connection().handle()) };
    let mut storage = sys::sockaddr_storage::default();
    let mut size = size_of::<sys::sockaddr_storage>() as sys::socklen_t;
    let address = (&mut storage as *mut sys::sockaddr_storage).cast();
    let read = unsafe { sys::lwip_getpeername(socket, address, &mut size) };
    if read != 0 {
        return Err("无法读取 USB 客户端地址".into());
    }
    let address = match u32::from(storage.ss_family) {
        sys::AF_INET => {
            let addr = unsafe { &*(&storage as *const _ as *const sys::sockaddr_in) };
            IpAddr::from(addr.sin_addr.s_addr.to_ne_bytes())
        }
        sys::AF_INET6 => {
            let addr = unsafe { &*(&storage as *const _ as *const sys::sockaddr_in6) };
            Ipv6Addr::from(unsafe { addr.sin6_addr.un.u8_addr }).to_canonical()
        }
        _ => return Err("无法读取 USB 客户端地址".into()),
    };
    match address {
        IpAddr::V4(ip) if matches!(ip.octets(), [192, 168, 72, 2..=254]) => Ok(address),
        _ => Err("请通过 USB 打开 192.168.72.1".into()),
    }
}

fn run() -> Result<()> {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();
    let _eventfs = MountedEventfs::mount(5)?;
    // Check that the runtime can create its network poller before exposing the buttons.
    drop(
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?,
    );
    let peripherals = Peripherals::take()?;
    let mut led = StatusLed::new(peripherals.pins.gpio48)
        .inspect_err(|error| log::warn!("MAPPIN_LED_DISABLED: {error}"))
        .ok();
    StatusLed::show(&mut led, Pattern::Starting, None, Instant::now());
    // Fail on an invalid partition before the wrapper can apply its erase-and-retry policy.
    EspError::convert(unsafe { sys::nvs_flash_init_partition(c"userdata".as_ptr()) })?;
    let partition = EspCustomNvsPartition::take("userdata")?;
    let pairing = EspKeyValueStorage::new(EspNvs::new(partition, "idevice", false)?);
    if !iphone::has_pairing_record(&pairing)? {
        return Err("现有配对资料不可用".into());
    }
    let session = Arc::new(Mutex::new(Session {
        pairing,
        controller: None,
        peer: None,
        restore_at: None,
    }));
    unsafe { mappin_usb_network_start() };
    let mut server = EspHttpServer::new(&Configuration {
        stack_size: 12 * 1024,
        max_open_sockets: 4,
        ..Default::default()
    })?;
    server.fn_handler("/", Method::Get, |request| -> Result<()> {
        request
            .into_response(
                200,
                None,
                &[
                    ("Content-Type", "text/html; charset=utf-8"),
                    ("Cache-Control", "no-store"),
                ],
            )?
            .write_all(include_bytes!("index.html"))?;
        Ok(())
    })?;
    for (path, set) in [("/set", true), ("/restore", false)] {
        let session = session.clone();
        server.fn_handler(path, Method::Post, move |mut request| -> Result<()> {
            let result = (|| -> Result<()> {
                if request.header("X-MapPin-Usb-Test") != Some("1") {
                    return Err("请使用测试页面的按钮".into());
                }
                let peer = usb_peer(&mut request)?;
                session
                    .lock()
                    .map_err(|_| "定位会话不可用")?
                    .command(peer, set)
            })();
            let (status, message) = match result {
                Ok(()) if set => (200, "已设置上海市中心。请在苹果地图确认。".into()),
                Ok(()) => (200, "已恢复真实定位。请在苹果地图确认。".into()),
                Err(error) => (502, error.to_string()),
            };
            request
                .into_response(
                    status,
                    None,
                    &[
                        ("Content-Type", "text/plain; charset=utf-8"),
                        ("Cache-Control", "no-store"),
                    ],
                )?
                .write_all(message.as_bytes())?;
            Ok(())
        })?;
    }
    StatusLed::show(&mut led, Pattern::Ready, None, Instant::now());
    log::info!(
        "MAPPIN_USB_LOCATION_READY http://{}",
        Ipv4Addr::new(192, 168, 72, 1)
    );
    loop {
        if let Ok(mut session) = session.try_lock() {
            session.restore_if_due();
        }
        thread::sleep(Duration::from_millis(250));
    }
}

fn main() {
    if let Err(error) = run() {
        log::error!("MAPPIN_USB_LOCATION_STARTUP_ERROR: {error}");
    }
}
