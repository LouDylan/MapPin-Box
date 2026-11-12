use std::{
    error::Error,
    fmt::{Debug, Display},
    sync::mpsc::{SyncSender, sync_channel},
    time::{Duration, Instant},
};

use enigma_embedded_core::Location;
use idevice::{
    RsdService,
    dvt::{location_simulation::LocationSimulationClient, remote_server::RemoteServerClient},
    remote_pairing::{
        RemotePairingClient, RpPairingFile, RpPairingSocket, connect_tls_psk_tunnel_native,
    },
    rsd::RsdHandshake,
};
use tokio::sync::mpsc;

use crate::sdk::{self as sys, EspError};

const REMOTE_PAIRING_HOST: &str = "mappin-box";
const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
// The pure-Rust TLS-PSK and tunneled TCP stacks use 16 KiB record buffers and
// deeply nested async futures. Keep this worker off scarce internal RAM and
// leave enough headroom for the complete CoreDevice session.
const SESSION_STACK_BYTES: usize = 256 * 1024;

enum LocationCommand {
    Check {
        deadline: Instant,
        reply: SyncSender<Result<(), String>>,
    },
    Set {
        latitude: f64,
        longitude: f64,
        deadline: Instant,
        reply: SyncSender<Result<(), String>>,
    },
    Clear {
        deadline: Instant,
        reply: SyncSender<Result<(), String>>,
    },
}

impl LocationCommand {
    fn fail(self, message: &str) {
        let result = Err(message.to_string());
        match self {
            Self::Check { reply, .. } | Self::Set { reply, .. } | Self::Clear { reply, .. } => {
                let _ = reply.send(result);
            }
        }
    }
}

pub struct IphoneController {
    commands: mpsc::Sender<LocationCommand>,
}

impl IphoneController {
    pub(crate) fn connect_remote(
        remote_pairing_file: RpPairingFile,
        address: std::net::IpAddr,
        discover: Option<fn(std::net::IpAddr) -> Result<u16, String>>,
    ) -> Result<Self, Box<dyn Error>> {
        let (commands, receiver) = mpsc::channel(4);
        spawn_worker(move || run_worker(address, remote_pairing_file, receiver, discover))?;
        log::info!("iPhone network session starting at {address}");
        Ok(Self { commands })
    }

    pub fn set(&mut self, location: &Location) -> Result<(), Box<dyn Error>> {
        let latitude = location.latitude.parse::<f64>()?;
        let longitude = location.longitude.parse::<f64>()?;
        self.request(|reply| LocationCommand::Set {
            latitude,
            longitude,
            deadline: Instant::now() + COMMAND_TIMEOUT,
            reply,
        })?;
        log::info!("location set successfully");
        Ok(())
    }

    /// Open the authenticated developer channel without issuing a location command.
    #[allow(dead_code)] // Used by the Bluetooth location probe.
    pub(crate) fn check(&self) -> Result<(), Box<dyn Error>> {
        self.request(|reply| LocationCommand::Check {
            reply,
            deadline: Instant::now() + COMMAND_TIMEOUT,
        })
    }

    pub fn restore(&mut self) -> Result<(), Box<dyn Error>> {
        self.request(|reply| LocationCommand::Clear {
            reply,
            deadline: Instant::now() + COMMAND_TIMEOUT,
        })?;
        log::info!("location simulation cleared; real location restored");
        Ok(())
    }

    fn request(
        &self,
        command: impl FnOnce(SyncSender<Result<(), String>>) -> LocationCommand,
    ) -> Result<(), Box<dyn Error>> {
        let (reply, response) = sync_channel(1);
        self.commands
            .blocking_send(command(reply))
            .map_err(|_| "iPhone network session stopped; tap again to reconnect")?;
        response
            .recv_timeout(COMMAND_TIMEOUT)
            .map_err(|_| "iPhone network session timed out; keep the phone unlocked and connected")?
            .map_err(Into::into)
    }
}

pub(crate) fn spawn_worker(worker: impl FnOnce() + Send + 'static) -> Result<(), Box<dyn Error>> {
    let mut previous = sys::esp_pthread_cfg_t::default();
    let previous_result = unsafe { sys::esp_pthread_get_cfg(&mut previous) };
    if previous_result == sys::ESP_ERR_NOT_FOUND {
        previous = unsafe { sys::esp_pthread_get_default_config() };
    } else {
        EspError::convert(previous_result)?;
    }

    let mut worker_config = unsafe { sys::esp_pthread_get_default_config() };
    worker_config.stack_size = SESSION_STACK_BYTES as _;
    worker_config.stack_alloc_caps = sys::MALLOC_CAP_8BIT | sys::MALLOC_CAP_SPIRAM;
    EspError::convert(unsafe { sys::esp_pthread_set_cfg(&worker_config) })?;

    let spawned = std::thread::Builder::new()
        .name("mappin-iphone".into())
        .stack_size(SESSION_STACK_BYTES)
        .spawn(worker);

    if let Err(error) = EspError::convert(unsafe { sys::esp_pthread_set_cfg(&previous) }) {
        log::warn!("could not restore default thread configuration: {error}");
    }

    spawned?;
    log::info!("iPhone worker allocated {SESSION_STACK_BYTES} bytes in PSRAM");
    Ok(())
}

fn run_worker(
    address: std::net::IpAddr,
    remote_pairing_file: RpPairingFile,
    mut receiver: mpsc::Receiver<LocationCommand>,
    discover: Option<fn(std::net::IpAddr) -> Result<u16, String>>,
) {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => {
            log::info!("iPhone async runtime ready");
            log_worker_stack("async runtime initialization");
            runtime
        }
        Err(error) => {
            log::error!("could not start iPhone runtime: {error}");
            fail_commands(
                &mut receiver,
                "iPhone runtime unavailable; restart board".into(),
            );
            return;
        }
    };

    if let Err(error) = runtime.block_on(run_remote_session(
        address,
        remote_pairing_file,
        &mut receiver,
        discover,
    )) {
        log::error!("iPhone network session failed: {error}");
        fail_commands(&mut receiver, error);
    }
}

async fn run_remote_session(
    address: std::net::IpAddr,
    mut pairing_file: RpPairingFile,
    receiver: &mut mpsc::Receiver<LocationCommand>,
    discover: Option<fn(std::net::IpAddr) -> Result<u16, String>>,
) -> Result<(), String> {
    // This phone's port 49152 was verified by the Mac Wi-Fi test. Direct
    // connection also works when iOS omits its mDNS advertisement on an AP.
    let stream = match tokio::time::timeout(
        Duration::from_secs(3),
        tokio::net::TcpStream::connect((address, 49152)),
    )
    .await
    {
        Ok(Ok(stream)) => {
            log::info!("iPhone remote pairing service connected at {address}:49152");
            stream
        }
        _ => {
            let discover =
                discover.ok_or("iPhone remote pairing service unreachable at port 49152")?;
            log::info!("discovering iPhone `_remotepairing._tcp` service");
            let pairing_port = discover(address)?;
            log::info!("iPhone remote pairing service found at {address}:{pairing_port}");
            tokio::net::TcpStream::connect((address, pairing_port))
                .await
                .map_err(|error| stage_error("connect to remote pairing service", error))?
        }
    };
    let socket = RpPairingSocket::new(stream);
    let mut pairing = RemotePairingClient::new(socket, REMOTE_PAIRING_HOST);
    pairing
        .attempt_pair_verify()
        .await
        .map_err(|error| stage_error("start remote pairing verification", error))?;
    pairing
        .validate_pairing(&mut pairing_file)
        .await
        .map_err(|error| stage_error("verify provisioned remote identity", error))?;
    let tunnel_port = pairing
        .create_tcp_listener()
        .await
        .map_err(|error| stage_error("request CoreDevice tunnel listener", error))?;
    log::info!("iPhone CoreDevice TLS-PSK tunnel announced port {tunnel_port}");
    let tunnel_stream = tokio::net::TcpStream::connect((address, tunnel_port))
        .await
        .map_err(|error| stage_error("connect to CoreDevice tunnel", error))?;
    log_worker_stack("TLS-PSK handshake");
    let tunnel = connect_tls_psk_tunnel_native(tunnel_stream, pairing.encryption_key())
        .await
        .map_err(|error| stage_error("secure CoreDevice tunnel", error))?;
    log_worker_stack("completed TLS-PSK tunnel");
    let rsd_port = tunnel.info.server_rsd_port;
    let our_ip = tunnel
        .info
        .client_address
        .parse::<std::net::IpAddr>()
        .map_err(|error| stage_error("parse CoreDevice client address", error))?;
    let their_ip = tunnel
        .info
        .server_address
        .parse::<std::net::IpAddr>()
        .map_err(|error| stage_error("parse CoreDevice server address", error))?;
    let mtu = tunnel.info.mtu as usize;
    let stream = tunnel.into_inner();
    let mut adapter = idevice::tcp::adapter::Adapter::new(Box::new(stream), our_ip, their_ip);
    adapter.set_mss(mtu.saturating_sub(60));
    let mut adapter = adapter.to_async_handle();
    let stream = adapter
        .connect(rsd_port)
        .await
        .map_err(|error| stage_error("connect to tunneled RSD", error))?;
    let mut handshake = RsdHandshake::new(stream)
        .await
        .map_err(|error| stage_error("perform tunneled RSD handshake", error))?;
    let mut server: RemoteServerClient<Box<dyn idevice::ReadWrite>> =
        RemoteServerClient::connect_rsd(&mut adapter, &mut handshake)
            .await
            .map_err(|error| stage_error("connect remote developer service", error))?;
    server
        .read_message(0)
        .await
        .map_err(|error| stage_error("start remote developer service", error))?;
    let mut location = LocationSimulationClient::new(&mut server)
        .await
        .map_err(|error| stage_error("open location simulation channel", error))?;
    log::info!("iPhone modern location service ready");

    while let Some(command) = receiver.recv().await {
        let remaining = match &command {
            LocationCommand::Check { deadline, .. }
            | LocationCommand::Set { deadline, .. }
            | LocationCommand::Clear { deadline, .. } => {
                deadline.saturating_duration_since(Instant::now())
            }
        };
        if remaining.is_zero() {
            command.fail("iPhone command timed out before execution");
            continue;
        }
        let timeout = remaining.min(Duration::from_secs(8));
        match command {
            LocationCommand::Check { reply, .. } => {
                let _ = reply.send(Ok(()));
            }
            LocationCommand::Set {
                latitude,
                longitude,
                reply,
                ..
            } => {
                let result = tokio::time::timeout(timeout, location.set(latitude, longitude))
                    .await
                    .map_err(|_| "iPhone location command timed out".to_string())
                    .and_then(|result| result.map_err(classify_error));
                let _ = reply.send(result);
            }
            LocationCommand::Clear { reply, .. } => {
                let result = tokio::time::timeout(timeout, location.clear())
                    .await
                    .map_err(|_| "iPhone restore command timed out".to_string())
                    .and_then(|result| result.map_err(classify_error));
                let _ = reply.send(result);
            }
        }
    }
    Ok(())
}

fn log_worker_stack(stage: &str) {
    let minimum_free = unsafe { sys::uxTaskGetStackHighWaterMark2(core::ptr::null_mut()) };
    log::info!("iPhone worker minimum free stack at {stage}: {minimum_free} bytes");
}

fn fail_commands(receiver: &mut mpsc::Receiver<LocationCommand>, error: String) {
    while let Some(command) = receiver.blocking_recv() {
        command.fail(&error);
    }
}

fn classify_error(error: impl Display + Debug) -> String {
    format!("{error} ({error:?})")
}

pub(super) fn stage_error(stage: &str, error: impl Display + Debug) -> String {
    format!("{stage}: {}", classify_error(error))
}
