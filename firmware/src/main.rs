//! Waypin Box firmware for the ESP32-S3 N16R8.

mod admin_pages;
mod app;
mod backend;
mod device_backup;
mod diagnostics;
mod iphone;
mod location_store;
mod map_settings;
#[path = "../../vendor/enigma/apps/embedded/platforms/esp-idf/src/persistent_storage.rs"]
mod persistent_storage;
mod serial_provision;
mod status_led;
mod wifi_access;
mod wifi_control;

mod portal;

use esp_idf_svc::{
    hal::{
        delay::FreeRtos,
        gpio::{AnyIOPin, PinDriver, Pull},
        peripherals::Peripherals,
        uart::{UartDriver, config::Config},
    },
    io::vfs::MountedEventfs,
    nvs::EspDefaultNvsPartition,
};
use waypin_box_model::{backup::BackupExport, button::Button, indicator::Pattern};
use status_led::StatusLed;
use std::{
    error::Error,
    sync::{Arc, Mutex},
    time::Instant,
};

fn run(led: &mut Option<StatusLed>) -> Result<(), Box<dyn Error>> {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();
    unsafe {
        esp_idf_svc::sys::esp_log_level_set(
            c"*".as_ptr(),
            esp_idf_svc::sys::esp_log_level_t_ESP_LOG_INFO,
        );
    }
    log::set_max_level(log::LevelFilter::Info);
    let peripherals = Peripherals::take()?;
    // The original LED demo configures its WS2812 on GPIO48 on these boards.
    *led = StatusLed::new(peripherals.pins.gpio48)
        .inspect_err(|error| log::warn!("WAYPIN_LED_DISABLED: {error}"))
        .ok();
    StatusLed::show(led, Pattern::Starting, None, Instant::now());
    let _eventfs = MountedEventfs::mount(5)?;
    let button = PinDriver::input(peripherals.pins.gpio0, Pull::Up)?;
    let uart = UartDriver::new(
        peripherals.uart0,
        peripherals.pins.gpio43,
        peripherals.pins.gpio44,
        Option::<AnyIOPin>::None,
        Option::<AnyIOPin>::None,
        &Config::default().rx_fifo_size(4096),
    )?;
    let partition = persistent_storage::take_and_migrate(EspDefaultNvsPartition::take()?)?;
    let app = app::App::start(partition.clone())?;
    serial_provision::start(uart, app.clone())?;
    let wifi = wifi_access::WifiAccessPoint::start(peripherals.modem, partition)?;
    let label = wifi.display_label();
    let address = wifi.address();
    let network = wifi_control::WifiControl::start(wifi, app.clone())?;
    let backup = Arc::new(Mutex::new(BackupExport::default()));
    let _server = portal::start(app.clone(), network.clone(), backup.clone())?;
    log::info!("WAYPIN_BOX_READY: {} | http://{}", label, address);
    log::info!(
        "BOOT: short press restores real location; hold 3 seconds to replay the last successful location"
    );
    let mut control = Button::default();
    let button_clock = Instant::now();
    let mut cache_clock = Instant::now();
    loop {
        if cache_clock.elapsed().as_secs() >= 60 {
            app.expire_cached_location();
            cache_clock = Instant::now();
        }
        let now = Instant::now();
        let action = control.update(
            button.is_low(),
            now.duration_since(button_clock).as_millis() as u64,
        );
        let (action, export) = backup
            .lock()
            .map(|mut backup| {
                let action = backup.button(control.is_pressed(), action, now);
                (action, backup.status(now))
            })
            .unwrap_or((action, None));
        if let Some(action) = action {
            app.button_action(action);
        }
        if led.is_some()
            && let Some(connected) = network.has_hotspot_client()
            && let Some((pattern, since)) = app.led_pattern(connected, export, now)
        {
            StatusLed::show(led, pattern, since, now);
        }
        FreeRtos::delay_ms(20);
    }
}

fn main() {
    let mut led = None;
    if let Err(error) = run(&mut led) {
        log::error!("WAYPIN_BOX_STARTUP_ERROR: {error}");
        loop {
            StatusLed::show(&mut led, Pattern::StartupFailed, None, Instant::now());
            FreeRtos::delay_ms(50);
        }
    }
}
