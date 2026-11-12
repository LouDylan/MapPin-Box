use crate::{
    backend::EspIdfBackend, iphone, location_store::LocationStore, map_settings::MapSettings,
    persistent_storage::PersistentNvsPartition,
};
use mappin_box_model::{
    Backup, MAX_FAVORITES, MAX_NAME, MAX_RECENT, MapChange, MapConfig, MapCredentials, Place,
    ReplayCache,
    backup::ExportStatus,
    button::Action,
    indicator::{Pattern, Status},
};
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    error::Error,
    net::IpAddr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{SyncSender, sync_channel},
    },
    time::Instant,
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

#[derive(Debug)]
pub(crate) struct PairingBusy;

impl std::fmt::Display for PairingBusy {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str("请先恢复真实定位并等待操作完成，再接入手机")
    }
}

impl Error for PairingBusy {}

#[derive(Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Kind {
    Set,
    Restore,
}

#[derive(Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Mode {
    Unknown,
    Simulating,
    Real,
}

#[derive(Clone)]
struct Operation {
    id: u32,
    kind: Kind,
    label: String,
    phase: &'static str,
    started: Instant,
}

impl Operation {
    fn json(&self) -> Value {
        json!({"id": self.id, "kind": self.kind, "label": self.label, "phase": self.phase,
            "elapsed_ms": self.started.elapsed().as_millis() as u64})
    }
}

#[derive(Serialize)]
struct Outcome {
    id: u32,
    ok: bool,
    message: String,
    detail: Option<String>,
}

struct State {
    revision: u64,
    next_id: u32,
    mode: Mode,
    location: Option<Place>,
    phone: Option<iphone::PhoneInfo>,
    replay_cache: ReplayCache,
    operation: Option<Operation>,
    queued_restore: Option<Operation>,
    result: Option<Outcome>,
    led_notice: Option<(Pattern, Instant)>,
    network_busy: bool,
    pairing_busy: bool,
    button: Option<Value>,
    button_id: u32,
    events: VecDeque<Value>,
    started: Instant,
}

impl Default for State {
    fn default() -> Self {
        Self {
            revision: 1,
            next_id: 1,
            mode: Mode::Unknown,
            location: None,
            phone: None,
            replay_cache: ReplayCache::default(),
            operation: None,
            queued_restore: None,
            result: None,
            led_notice: None,
            network_busy: false,
            pairing_busy: false,
            button: None,
            button_id: 0,
            events: VecDeque::new(),
            started: Instant::now(),
        }
    }
}

impl State {
    fn check_phone_idle(&self) -> std::result::Result<(), PairingBusy> {
        if self.operation.is_some() || self.network_busy || self.mode == Mode::Simulating {
            return Err(PairingBusy);
        }
        Ok(())
    }

    fn event(&mut self, level: &str, message: &str) {
        if level == "error" {
            self.led_notice = Some((Pattern::Failed, Instant::now()));
        }
        if self.events.len() == 32 {
            self.events.pop_front();
        }
        let message: String = message.chars().take(180).collect();
        self.events.push_back(
            json!({"uptime": self.started.elapsed().as_secs(), "level": level, "message": message}),
        );
    }
}

struct Job {
    operation: Operation,
    address: Option<IpAddr>,
    place: Option<Place>,
    replay: bool,
}

pub struct App {
    state: Arc<Mutex<State>>,
    data: Arc<Mutex<LocationStore>>,
    pairing: Mutex<iphone::PairingStorage>,
    maps: Mutex<MapSettings>,
    paired: AtomicBool,
    commands: SyncSender<Job>,
    started: Instant,
    boot_id: u32,
}

impl App {
    pub fn start(partition: PersistentNvsPartition) -> Result<Arc<Self>> {
        let backend = EspIdfBackend::new(partition.clone())?;
        let pairing = iphone::pairing_storage(partition.clone())?;
        let (paired, phone) = iphone::SavedPairing::inspect(&pairing)?;
        log::info!(
            "MAPPIN_DEVELOPER_PAIRING {}",
            if paired { "saved" } else { "not configured" }
        );
        let maps = Mutex::new(MapSettings::new(partition.clone())?);
        let data = Arc::new(Mutex::new(LocationStore::new(partition)?));
        let state = Arc::new(Mutex::new(State {
            phone,
            ..Default::default()
        }));
        let (commands, receiver) = sync_channel(2);
        let worker_state = state.clone();
        let worker_data = data.clone();
        std::thread::Builder::new()
            .name("mappin-actions".into())
            .stack_size(16 * 1024)
            .spawn(move || run_worker(backend, receiver, worker_state, worker_data))?;
        Ok(Arc::new(Self {
            state,
            data,
            pairing: Mutex::new(pairing),
            maps,
            paired: AtomicBool::new(paired),
            commands,
            started: Instant::now(),
            boot_id: unsafe { esp_idf_svc::sys::esp_random() },
        }))
    }

    fn is_paired(&self) -> bool {
        self.paired.load(Ordering::Acquire)
    }

    pub fn led_pattern(
        &self,
        connected: bool,
        backup: Option<ExportStatus>,
        now: Instant,
    ) -> Option<(Pattern, Option<Instant>)> {
        let state = self.state.try_lock().ok()?;
        let working = state.operation.is_some() || state.network_busy || state.pairing_busy;
        if !working
            && let Some((notice, since)) = state.led_notice
            && notice.color(now.saturating_duration_since(since)).is_some()
        {
            return Some((notice, Some(since)));
        }
        let status = Status {
            paired: self.paired.load(Ordering::Acquire),
            connected,
            working,
            simulating: state.mode == Mode::Simulating,
            backup,
        };
        Some((status.pattern(), None))
    }

    pub fn snapshot(&self) -> Result<Value> {
        let paired = self.is_paired();
        let maps = self.map_config()?;
        let maps_configured = maps.configured();
        let state = self.state.lock().map_err(|_| "状态暂不可用")?;
        let data = self.data.lock().map_err(|_| "地点暂不可用")?;
        Ok(
            json!({"version": env!("CARGO_PKG_VERSION"), "boot_id": self.boot_id, "revision": state.revision,
            "paired": paired, "phone": state.phone, "maps_configured": maps_configured, "map_provider": maps.provider, "configured_providers": maps.available(), "regional_providers": maps.regional_providers(), "uptime": self.started.elapsed().as_secs(), "mode": state.mode,
            "location": state.location, "operation": state.operation.as_ref().map(Operation::json),
            "restore_queued": state.queued_restore.is_some(), "result": state.result,
            "network_busy": state.network_busy, "pairing_busy": state.pairing_busy, "button": state.button,
            "last_location": data.recent.first(),
            "favorites": data.favorites.items, "recent": data.recent,
            "limits": {"favorites": MAX_FAVORITES, "recent": MAX_RECENT, "name": MAX_NAME}}),
        )
    }

    pub fn map_config(&self) -> Result<MapConfig> {
        Ok(self.maps.lock().map_err(|_| "地图配置暂不可用")?.get())
    }

    pub fn export_backup(&self) -> Result<Vec<u8>> {
        // The HTTP server serializes map and favorite writes. Holding state also
        // excludes serial pairing, location jobs and Wi-Fi configuration changes.
        let state = self.state.lock().map_err(|_| "状态暂不可用")?;
        if state.operation.is_some() || state.network_busy || state.pairing_busy {
            return Err("请等当前操作结束后再导出备份".into());
        }
        crate::device_backup::capture()
    }

    pub fn save_map_config(&self, value: MapChange) -> Result<()> {
        self.maps
            .lock()
            .map_err(|_| "地图配置暂不可用")?
            .save(value)?;
        self.changed();
        Ok(())
    }

    pub fn seed_amap(&self, credentials: MapCredentials) -> Result<bool> {
        // Serial writes must not race the raw userdata backup.
        let mut state = self.state.lock().map_err(|_| "状态暂不可用")?;
        let written = self
            .maps
            .lock()
            .map_err(|_| "地图配置暂不可用")?
            .seed_amap(credentials)?;
        if written {
            state.revision += 1;
        }
        Ok(written)
    }

    pub fn import_pairing(&self, bytes: &[u8]) -> Result<()> {
        let pairing = iphone::SavedPairing::serial(bytes)?;
        self.replace_pairing(pairing, false)
    }

    #[cfg(esp32)]
    pub fn can_change_phone(&self) -> Result<()> {
        let state = self.state.lock().map_err(|_| "状态暂不可用")?;
        if state.pairing_busy {
            return Err("正在配对 iPhone，请稍候".into());
        }
        Ok(state.check_phone_idle()?)
    }

    #[cfg(esp32)]
    pub fn begin_pairing(&self) -> Result<()> {
        let mut state = self.state.lock().map_err(|_| "状态暂不可用")?;
        if state.pairing_busy {
            return Err("正在配对 iPhone，请稍候".into());
        }
        state.check_phone_idle()?;
        state.pairing_busy = true;
        state.revision += 1;
        Ok(())
    }

    #[cfg(esp32)]
    pub fn end_pairing(&self) {
        let mut state = self.state.lock().unwrap();
        state.pairing_busy = false;
        state.revision += 1;
    }

    pub fn replace_pairing(&self, record: iphone::SavedPairing, native: bool) -> Result<()> {
        let mut state = self.state.lock().map_err(|_| "状态暂不可用")?;
        if state.pairing_busy != native {
            return Err(PairingBusy.into());
        }
        state.check_phone_idle()?;
        let pairing = self.pairing.lock().map_err(|_| "配对资料暂不可用")?;
        let mut data = self.data.lock().map_err(|_| "地点暂不可用")?;
        data.forget_phone()?;
        record.save(&pairing)?;
        state.phone = record.phone;
        state.mode = Mode::Unknown;
        state.location = None;
        state.result = None;
        state.led_notice = None;
        state.button = None;
        state.replay_cache = ReplayCache::default();
        state.revision += 1;
        state.event("info", "已接入手机，请在手机网页完成首次连接验证");
        self.paired.store(true, Ordering::Release);
        Ok(())
    }

    pub fn set(&self, address: IpAddr, place: Place) -> Result<u32> {
        let place = place.validate()?;
        place.point()?;
        self.submit(Kind::Set, Some(address), Some(place), false)
    }

    pub fn restore(&self, address: Option<IpAddr>) -> Result<u32> {
        let address = address.or_else(|| {
            let last = self.data.lock().ok()?.phone_ip;
            self.default_address(last)
        });
        self.submit(Kind::Restore, address, None, false)
    }

    pub fn replay_last(&self, address: Option<IpAddr>) -> Result<u32> {
        let (place, last) = {
            let data = self.data.lock().map_err(|_| "地点暂不可用")?;
            (
                data.recent
                    .first()
                    .cloned()
                    .ok_or("还没有成功定位记录，请先在网页设置一次定位")?,
                data.phone_ip,
            )
        };
        let place = {
            let mut state = self.state.lock().map_err(|_| "状态暂不可用")?;
            let now = state.started.elapsed().as_secs();
            state.replay_cache.resolve(place, now)?
        };
        self.submit(
            Kind::Set,
            address.or_else(|| self.default_address(last)),
            Some(place),
            true,
        )
    }

    fn default_address(&self, last: Option<IpAddr>) -> Option<IpAddr> {
        #[cfg(esp32)]
        {
            let bluetooth = crate::bluetooth::peer();
            let wifi = last.filter(|address| mappin_box_model::control_client(*address));
            // Restore on the original path when it still holds the simulation.
            if self
                .state
                .lock()
                .is_ok_and(|state| state.mode == Mode::Simulating)
            {
                return last
                    .filter(|address| Some(*address) == bluetooth)
                    .or(wifi)
                    .or(bluetooth);
            }
            bluetooth.or(wifi)
        }
        #[cfg(not(esp32))]
        last
    }

    pub fn expire_cached_location(&self) {
        let mut state = self.state.lock().unwrap();
        let now = state.started.elapsed().as_secs();
        state.replay_cache.expire(now);
    }

    pub fn button_action(&self, action: Action) {
        let (kind, result) = match action {
            Action::Restore => ("restore", self.restore(None)),
            Action::Replay => ("replay", self.replay_last(None)),
        };
        let mut state = self.state.lock().unwrap();
        state.button_id += 1;
        let message = match &result {
            Ok(_) => {
                if action == Action::Restore {
                    "BOOT 短按：已提交恢复真实定位".into()
                } else {
                    "BOOT 长按：已提交上次地点定位".into()
                }
            }
            Err(error) => error.to_string(),
        };
        state.button = Some(
            json!({"id": state.button_id, "kind": kind, "ok": result.is_ok(), "message": message, "uptime": self.started.elapsed().as_secs()}),
        );
        state.event(if result.is_ok() { "info" } else { "error" }, &message);
        state.revision += 1;
    }

    pub fn begin_network_change(&self) -> Result<()> {
        let mut state = self.state.lock().map_err(|_| "状态暂不可用")?;
        if state.pairing_busy {
            return Err("正在配对 iPhone，请稍候".into());
        }
        if state.network_busy {
            return Err("已有 Wi-Fi 操作正在执行，请稍候".into());
        }
        if state.operation.is_some() || state.mode == Mode::Simulating {
            return Err("请先恢复真实定位，再扫描或更改 Wi-Fi".into());
        }
        state.network_busy = true;
        state.led_notice = None;
        state.revision += 1;
        Ok(())
    }

    pub fn end_network_change(&self) {
        let mut state = self.state.lock().unwrap();
        state.network_busy = false;
        state.revision += 1;
    }

    pub fn event(&self, level: &str, message: &str) {
        self.state.lock().unwrap().event(level, message);
    }

    pub fn debug_state(&self) -> Result<Value> {
        let state = self.state.lock().map_err(|_| "状态暂不可用")?;
        let data = self.data.lock().map_err(|_| "地点暂不可用")?;
        Ok(json!({"events": state.events, "phone_ip": data.phone_ip}))
    }

    pub fn clear_events(&self) {
        self.state.lock().unwrap().events.clear();
    }

    fn submit(
        &self,
        kind: Kind,
        address: Option<IpAddr>,
        place: Option<Place>,
        replay: bool,
    ) -> Result<u32> {
        if !self.is_paired() {
            return Err("请先在「设置 → 使用帮助」接入 iPhone".into());
        }
        let mut state = self.state.lock().map_err(|_| "状态暂不可用")?;
        if state.pairing_busy {
            return Err("正在配对 iPhone，完成后再操作定位".into());
        }
        if state.network_busy {
            return Err("正在调整 Wi-Fi，完成后再操作定位".into());
        }
        if let Some(current) = &state.operation {
            if kind == Kind::Set {
                return Err("正在执行上一项操作，请稍候再切换地点".into());
            }
            if current.kind == Kind::Restore {
                return Ok(current.id);
            }
            if let Some(queued) = &state.queued_restore {
                return Ok(queued.id);
            }
        }
        let operation = Operation {
            id: state.next_id,
            kind,
            label: place
                .as_ref()
                .map(|item| {
                    if item.source == "google" {
                        item.query.clone()
                    } else {
                        item.name.clone()
                    }
                })
                .unwrap_or_else(|| "真实定位".into()),
            phase: "queued",
            started: Instant::now(),
        };
        let id = operation.id;
        self.commands
            .try_send(Job {
                operation: operation.clone(),
                address,
                place,
                replay,
            })
            .map_err(|_| "操作队列繁忙，请稍后重试")?;
        state.next_id = state.next_id.wrapping_add(1).max(1);
        state.led_notice = None;
        if state.operation.is_some() {
            state.queued_restore = Some(operation);
        } else {
            state.operation = Some(operation);
        }
        state.revision += 1;
        state.event(
            "info",
            if kind == Kind::Set {
                "已提交定位操作"
            } else {
                "已提交恢复真实定位"
            },
        );
        Ok(id)
    }

    pub fn save(&self, id: Option<u32>, place: Place) -> Result<u32> {
        let id = self
            .data
            .lock()
            .map_err(|_| "地点暂不可用")?
            .save(id, place)?;
        self.changed();
        Ok(id)
    }

    pub fn delete(&self, id: u32) -> Result<()> {
        self.data.lock().map_err(|_| "地点暂不可用")?.delete(id)?;
        self.changed();
        Ok(())
    }

    pub fn import(&self, backup: Backup) -> Result<(usize, usize)> {
        let counts = self
            .data
            .lock()
            .map_err(|_| "地点暂不可用")?
            .import(backup)?;
        self.changed();
        Ok(counts)
    }

    pub fn clear_recent(&self) -> Result<()> {
        self.data
            .lock()
            .map_err(|_| "地点暂不可用")?
            .clear_recent()?;
        self.changed();
        Ok(())
    }

    fn changed(&self) {
        self.state.lock().unwrap().revision += 1;
    }
}

fn run_worker(
    mut backend: EspIdfBackend,
    receiver: std::sync::mpsc::Receiver<Job>,
    state: Arc<Mutex<State>>,
    data: Arc<Mutex<LocationStore>>,
) {
    for mut job in receiver {
        job.operation.phase = "working";
        job.operation.started = Instant::now();
        {
            let mut state = state.lock().unwrap();
            if state
                .queued_restore
                .as_ref()
                .is_some_and(|item| item.id == job.operation.id)
            {
                state.queued_restore = None;
            }
            state.operation = Some(job.operation.clone());
            state.revision += 1;
        }
        if let Some(address) = job.address {
            backend.select_phone(address);
        }
        let result = match &job.place {
            Some(place) => backend.set_location(place),
            None => backend.restore_location(),
        };
        let outcome = complete(&job, result, &data);
        let mut state = state.lock().unwrap();
        match (outcome.ok, &job.place) {
            (true, Some(place)) => {
                state.mode = Mode::Simulating;
                state.location = place.clone().stored().ok();
                let now = state.started.elapsed().as_secs();
                if !job.replay {
                    state.replay_cache.remember(place, now);
                }
            }
            (true, None) => {
                state.mode = Mode::Real;
                state.location = None;
                state.led_notice = Some((Pattern::Restored, Instant::now()));
            }
            (false, _) => {
                state.mode = Mode::Unknown;
            }
        }
        state.result = Some(outcome);
        let message = state.result.as_ref().unwrap().message.clone();
        let level = if state.result.as_ref().unwrap().ok {
            "info"
        } else {
            "error"
        };
        state.event(level, &message);
        state.operation = state.queued_restore.clone();
        state.revision += 1;
    }
}

fn complete(job: &Job, result: Result<()>, data: &Mutex<LocationStore>) -> Outcome {
    let id = job.operation.id;
    if let Err(error) = result {
        let detail = error.to_string();
        log::warn!("MAPPIN_ACTION_ERROR {id}: {detail}");
        return Outcome {
            id,
            ok: false,
            message: explain_error(&detail).into(),
            detail: Some(detail),
        };
    }
    let mut message = match &job.place {
        Some(_) => format!("已切换到“{}”", job.operation.label),
        None => "已恢复真实定位".into(),
    };
    if let Some(address) = job.address {
        if let Err(error) = data.lock().unwrap().remember_phone(address) {
            log::warn!("Phone address could not be saved: {error}");
            message.push_str("，但手机地址未能保存");
        }
    }
    if let Some(place) = &job.place {
        if let Err(error) = data.lock().unwrap().record(place.clone()) {
            log::warn!("location set, but history could not be saved: {error}");
            message.push_str("，但最近记录未能保存");
        }
    }
    log::info!("MAPPIN_ACTION_OK {id}: {message}");
    Outcome {
        id,
        ok: true,
        message,
        detail: None,
    }
}

fn explain_error(detail: &str) -> &str {
    let error = detail.to_lowercase();
    if error.contains("请先") {
        return detail;
    }
    if error.contains("pairing is missing") {
        return "缺少手机配对资料，请在「设置 → 使用帮助」接入 iPhone";
    }
    if error.contains("timeout") || error.contains("timed out") {
        return "连接 iPhone 超时。请保持手机解锁，确认蓝牙或备用 Wi-Fi 仍连通";
    }
    if error.contains("verify") || error.contains("signature") || error.contains("pairing") {
        return "未能验证 iPhone 配对。请解锁后重试，或在「使用帮助」重新配对";
    }
    if error.contains("developer") || error.contains("rsd") || error.contains("location simulation")
    {
        return "未能打开 iPhone 定位服务。解锁后重试；仍失败时接 Mac 运行“准备iPhone定位服务”";
    }
    "未能完成定位操作。请保持 iPhone 解锁并连接盒子，然后重试"
}
