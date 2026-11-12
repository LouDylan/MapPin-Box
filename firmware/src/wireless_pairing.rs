use crate::{
    app::App,
    bluetooth::{self, Endpoint},
    iphone::{PhoneInfo, SavedPairing},
    iphone_session::spawn_worker,
    sdk,
};
use esp_idf_svc::mdns::EspMdns;
use idevice::remote_pairing::{PairableHost, PairableHostInfo, RpPairingFile, RpPairingSocket};
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    error::Error,
    net::IpAddr,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{net::TcpListener, time};

type Result<T> = std::result::Result<T, Box<dyn Error>>;
const WINDOW: Duration = Duration::from_secs(300);

#[derive(Serialize)]
#[serde(tag = "stage", rename_all = "snake_case")]
enum Stage {
    Idle,
    Preparing,
    Waiting { name: String },
    Pin { name: String, code: String },
    Saving,
    Done,
    Failed { message: String },
}

struct Attempt {
    endpoint: Option<Endpoint>,
    stage: Stage,
    pending: Option<SavedPairing>,
}

static ATTEMPT: Mutex<Attempt> = Mutex::new(Attempt {
    endpoint: None,
    stage: Stage::Idle,
    pending: None,
});

pub fn snapshot(endpoint: Endpoint) -> Result<Value> {
    let attempt = ATTEMPT.lock().map_err(|_| "配对状态暂不可用")?;
    Ok(if attempt.endpoint == Some(endpoint) {
        json!(attempt.stage)
    } else {
        json!(Stage::Idle)
    })
}

pub fn start(app: Arc<App>, endpoint: Endpoint) -> Result<()> {
    app.begin_pairing()?;
    let result = (|| {
        let mut attempt = ATTEMPT.lock().map_err(|_| "配对状态暂不可用")?;
        attempt.endpoint = Some(endpoint);
        attempt.stage = Stage::Preparing;
        let app = app.clone();
        spawn_worker(move || {
            let result = (|| -> Result<()> {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()?;
                let record = runtime.block_on(accept(endpoint))?;
                let mut attempt = ATTEMPT.lock().map_err(|_| "配对状态暂不可用")?;
                attempt.stage = Stage::Saving;
                attempt.pending = Some(record);
                Ok(())
            })();
            if let Err(error) = result {
                finish(&app, Err(error));
            }
        })
    })();
    if let Err(error) = result {
        finish(&app, Err(error.to_string().into()));
        return Err(error);
    }
    Ok(())
}

fn finish(app: &App, result: Result<()>) {
    if let Ok(mut attempt) = ATTEMPT.lock() {
        attempt.stage = match result {
            Ok(()) => {
                log::info!("MAPPIN_BT_PAIR_DONE");
                Stage::Done
            }
            Err(error) => {
                log::warn!("MAPPIN_BT_PAIR_FAILED: {error}");
                Stage::Failed {
                    message: error.to_string(),
                }
            }
        };
    }
    app.end_pairing();
}

// ESP32 pauses access to PSRAM for flash writes. Only the main task commits.
pub fn commit_pending(app: &App) {
    let record = ATTEMPT
        .try_lock()
        .ok()
        .and_then(|mut attempt| attempt.pending.take());
    if let Some(record) = record {
        finish(app, app.replace_pairing(record, true));
    }
}

async fn accept(endpoint: Endpoint) -> Result<SavedPairing> {
    let listener = TcpListener::bind((endpoint.local, 0)).await?;
    let seed = format!("{:08x}{:08x}", unsafe { sdk::esp_random() }, unsafe {
        sdk::esp_random()
    });
    let mut record = RpPairingFile::generate(&seed);
    record.identifier.make_ascii_uppercase();
    let id = record.identifier();
    let name = format!("MapPin-Box {}", &id[..4]);
    let info = PairableHostInfo::generate(&name, "Mac17,7");
    let port = listener.local_addr()?.port();
    let mut mdns = EspMdns::take()?;
    bluetooth::enable_mdns()?;
    mdns.set_hostname(format!("mappin-bt-{}", &id[..8]))?;
    let txt = info.mdns_txt_records(id);
    let txt = txt
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect::<Vec<_>>();
    mdns.add_service(Some(id), "_remotepairing-pairable-host", "_tcp", port, &txt)?;
    ATTEMPT.lock().map_err(|_| "配对状态暂不可用")?.stage = Stage::Waiting { name: name.clone() };
    log::info!("MAPPIN_BT_PAIR_WAITING port={port} name={name}");

    let handshake = async {
        let stream = loop {
            let (stream, remote) = listener.accept().await?;
            if remote.ip() == IpAddr::V4(endpoint.peer) {
                break stream;
            }
        };
        log::info!("MAPPIN_BT_PAIR_CONNECTED");
        let mut host = PairableHost::new(RpPairingSocket::new_device(stream), info);
        let phone = host
            .accept(&mut record, |code| async move {
                if let Ok(mut attempt) = ATTEMPT.lock() {
                    attempt.stage = Stage::Pin { name, code };
                    log::info!("MAPPIN_BT_PAIR_CODE_READY");
                }
            })
            .await?;
        let clean = |text: &str| text.chars().filter(|c| !c.is_control()).take(64).collect();
        Ok(SavedPairing::native(
            &record,
            PhoneInfo {
                name: clean(&phone.name),
                product_type: clean(&phone.model),
                product_version: String::new(),
            },
        ))
    };
    tokio::select! {
        result = time::timeout(WINDOW, handshake) => result.map_err(|_| "配对已超时，请重新开始")?,
        () = disconnected(endpoint) => Err("蓝牙连接已断开，请重新连接后配对".into()),
    }
}

async fn disconnected(expected: Endpoint) {
    while bluetooth::endpoint() == Some(expected) {
        time::sleep(Duration::from_secs(1)).await;
    }
}
