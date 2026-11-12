use crate::{
    Result, SESSION, Session, iphone_session, sdk, mappin_location_mdns_enable,
    mappin_location_mdns_report, mappin_pair_trace, mappin_setup_address, mappin_setup_ifindex,
    mappin_setup_sta_address, mappin_setup_stop,
};
use idevice::remote_pairing::{PairableHost, PairableHostInfo, RpPairingFile, RpPairingSocket};
use sdk::EspError;
use std::{
    ffi::CString,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::net::TcpListener;

static ATTEMPT: Mutex<Attempt> = Mutex::new(Attempt {
    endpoint: None,
    stage: Stage::Idle,
});
static PENDING: Mutex<Option<(RpPairingFile, bool)>> = Mutex::new(None);
static SAVED: AtomicBool = AtomicBool::new(false);

struct Attempt {
    endpoint: Option<(Ipv4Addr, IpAddr)>,
    stage: Stage,
}

enum Stage {
    Idle,
    Preparing,
    Waiting { name: String },
    Pin { name: String, code: String },
    Saving,
    Done,
    Failed(String),
}

impl Stage {
    fn busy(&self) -> bool {
        matches!(
            self,
            Self::Preparing | Self::Waiting { .. } | Self::Pin { .. } | Self::Saving
        )
    }
}

pub fn busy() -> Result<bool> {
    Ok(ATTEMPT.lock().map_err(|_| "配对状态不可用")?.stage.busy())
}

pub fn set_saved(saved: bool) {
    SAVED.store(saved, Ordering::Release);
}

fn idle_message() -> String {
    if SAVED.load(Ordering::Acquire) {
        "已保存开发者配对".into()
    } else {
        "需要首次接入时，点「开始配对」。".into()
    }
}

pub fn message(local: Ipv4Addr, peer: IpAddr) -> Result<String> {
    let attempt = ATTEMPT.lock().map_err(|_| "配对状态不可用")?;
    if attempt.endpoint != Some((local, peer)) {
        return Ok(idle_message());
    }
    Ok(match &attempt.stage {
        Stage::Idle => idle_message(),
        Stage::Preparing => "正在启动配对…".into(),
        Stage::Waiting { name } => format!("盒子名称：{name}\n等待 iPhone 发起配对…"),
        Stage::Pin { name, code } => format!("盒子名称：{name}\n本次验证码：{code}"),
        Stage::Saving => "正在保存配对资料…".into(),
        Stage::Done => "开发者配对已完成。可以尝试设置定位。".into(),
        Stage::Failed(error) => format!("配对未完成：{error}"),
    })
}

pub fn start(local: Ipv4Addr, peer: IpAddr) -> Result<()> {
    let on_setup = u32::from_ne_bytes(local.octets()) == unsafe { mappin_setup_address() };
    let advertised = if on_setup {
        let sta = unsafe { mappin_setup_sta_address() };
        if sta == 0 {
            return Err("请先让盒子加入你平时用的 Wi-Fi".into());
        }
        Ipv4Addr::from(sta.to_ne_bytes())
    } else {
        local
    };
    let session = SESSION.get().ok_or("定位会话尚未启动")?;
    let state = session.lock().map_err(|_| "定位会话不可用")?;
    if state.restore_at.is_some() {
        return Err("请先恢复真实定位".into());
    }
    let mut attempt = ATTEMPT.lock().map_err(|_| "配对状态不可用")?;
    if attempt.stage.busy() {
        if attempt.endpoint != Some((local, peer)) {
            return Err("另一条连接正在配对".into());
        }
        return Ok(());
    }
    attempt.endpoint = Some((local, peer));
    attempt.stage = Stage::Preparing;
    let start = iphone_session::spawn_worker(move || {
        let result = (|| -> Result<()> {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            let record = runtime.block_on(accept(advertised, peer, on_setup))?;
            ATTEMPT.lock().map_err(|_| "配对状态不可用")?.stage = Stage::Saving;
            *PENDING.lock().map_err(|_| "无法暂存配对资料")? = Some((record, on_setup));
            Ok(())
        })();
        if let Err(error) = result {
            failed(error.to_string());
        }
    });
    if let Err(error) = start {
        attempt.stage = Stage::Failed(error.to_string());
        return Err(error);
    }
    Ok(())
}

fn failed(error: String) {
    log::warn!("MAPPIN_BT_PAIR_FAILED: {error}");
    if let Ok(mut attempt) = ATTEMPT.lock() {
        attempt.stage = Stage::Failed(error);
    }
}

/// The ESP32 disables external RAM access when writing flash. The main task
/// owns persistence; the handshake worker's stack is in PSRAM.
pub fn commit_pending(state: &mut Session) {
    let Some((record, close_setup)) = PENDING.lock().ok().and_then(|mut pending| pending.take())
    else {
        return;
    };
    if let Err(error) = state.pairing.save_pairing(&record) {
        failed(error.to_string());
        return;
    }
    state.temporary_pairing = None;
    state.controller = None;
    set_saved(true);
    log::info!("MAPPIN_BT_PAIR_DONE");
    if let Ok(mut attempt) = ATTEMPT.lock() {
        attempt.stage = Stage::Done;
    }
    if close_setup {
        unsafe { mappin_setup_stop() };
    }
}

struct Advertisement(u32);

impl Advertisement {
    fn start(
        host: &PairableHostInfo,
        identifier: &str,
        local: Ipv4Addr,
        port: u16,
    ) -> Result<Self> {
        EspError::convert(unsafe { sdk::mdns_init() })?;
        let advertisement = Self(u32::from_ne_bytes(local.octets()));
        EspError::convert(unsafe { mappin_location_mdns_enable(advertisement.0) })?;
        let hostname = CString::new(format!("mappin-bt-{}", &identifier[..8]))?;
        EspError::convert(unsafe { sdk::mdns_hostname_set(hostname.as_ptr()) })?;
        let txt = host
            .mdns_txt_records(identifier)
            .into_iter()
            .map(|(key, value)| Ok((CString::new(key)?, CString::new(value)?)))
            .collect::<Result<Vec<_>>>()?;
        let mut txt = txt
            .iter()
            .map(|(key, value)| sdk::mdns_txt_item_t {
                key: key.as_ptr(),
                value: value.as_ptr(),
            })
            .collect::<Vec<_>>();
        let instance = CString::new(identifier)?;
        EspError::convert(unsafe {
            sdk::mdns_service_add(
                instance.as_ptr(),
                c"_remotepairing-pairable-host".as_ptr(),
                c"_tcp".as_ptr(),
                port,
                txt.as_mut_ptr(),
                txt.len(),
            )
        })?;
        Ok(advertisement)
    }
}

impl Drop for Advertisement {
    fn drop(&mut self) {
        unsafe {
            mappin_pair_trace(0);
            mappin_location_mdns_report(self.0);
            sdk::mdns_free();
        }
    }
}

async fn accept(local: Ipv4Addr, peer: IpAddr, lan: bool) -> Result<RpPairingFile> {
    let wifi_index = unsafe { mappin_setup_ifindex(u32::from_ne_bytes(local.octets())) };
    let listen_ip = if wifi_index > 0 {
        IpAddr::V6(Ipv6Addr::UNSPECIFIED)
    } else {
        IpAddr::V4(local)
    };
    let listener = TcpListener::bind((listen_ip, 0)).await?;
    let seed = format!(
        "mappin-pan-{:08x}{:08x}",
        unsafe { sdk::esp_random() },
        unsafe { sdk::esp_random() }
    );
    let mut record = RpPairingFile::generate(&seed);
    record.identifier.make_ascii_uppercase();
    let name = format!("MapPin-Box {}", &record.identifier()[..4]);
    let host = PairableHostInfo::generate(&name, "Mac17,7");
    let port = listener.local_addr()?.port();
    let _advertisement = Advertisement::start(&host, record.identifier(), local, port)?;
    unsafe { mappin_pair_trace(port) };
    log::info!("MAPPIN_BT_PAIR_WAITING address={local} port={port} name={name}");
    ATTEMPT.lock().map_err(|_| "配对状态不可用")?.stage = Stage::Waiting { name: name.clone() };
    tokio::time::timeout(Duration::from_secs(300), async {
        let stream = loop {
            let (stream, address) = listener.accept().await?;
            let local_wifi = matches!(address, SocketAddr::V6(ip)
                if wifi_index > 0 && ip.ip().is_unicast_link_local() && ip.scope_id() == wifi_index as u32);
            let ipv4_matches = stream.local_addr()?.ip().to_canonical() == IpAddr::V4(local)
                && (lan || address.ip().to_canonical() == peer);
            if local_wifi || ipv4_matches {
                break stream;
            }
            log::warn!("MAPPIN_PAIR_UNEXPECTED_PEER");
        };
        log::info!("MAPPIN_BT_PAIR_CONNECTED");
        let mut host = PairableHost::new(RpPairingSocket::new_device(stream), host);
        host.accept(&mut record, |code| async move {
            if let Ok(mut attempt) = ATTEMPT.lock() {
                attempt.stage = Stage::Pin { name, code };
                log::info!("MAPPIN_BT_PAIR_CODE_READY");
            }
        }).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })
    .await
    .map_err(|_| "iPhone 未在配对窗口内完成确认")??;
    Ok(record)
}
