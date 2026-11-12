use crate::{app::App, wifi_access::WifiAccessPoint};
use waypin_box_model::{HotspotChange, WifiCredentials};
use serde_json::{Value, json};
use std::{
    error::Error,
    sync::{
        Arc, Mutex,
        mpsc::{self, SyncSender},
    },
    time::{Duration, Instant},
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

pub enum Command {
    Scan,
    Connect(WifiCredentials),
    Disconnect,
    Hotspot(HotspotChange),
}

impl Command {
    fn name(&self) -> &'static str {
        match self {
            Self::Scan => "scan",
            Self::Connect(_) => "connect",
            Self::Disconnect => "disconnect",
            Self::Hotspot(_) => "hotspot",
        }
    }
}

struct Operation {
    id: u32,
    kind: &'static str,
    started: Instant,
}

struct State {
    live: Value,
    networks: Vec<Value>,
    operation: Option<Operation>,
    result: Option<Value>,
    next_id: u32,
}

#[derive(Clone)]
pub struct WifiControl {
    state: Arc<Mutex<State>>,
    commands: SyncSender<(u32, Command)>,
    app: Arc<App>,
}

impl WifiControl {
    pub fn start(wifi: WifiAccessPoint, app: Arc<App>) -> Result<Self> {
        let saved = wifi.saved_credentials();
        let state = Arc::new(Mutex::new(State {
            live: wifi.snapshot(),
            networks: Vec::new(),
            operation: None,
            result: None,
            next_id: 1,
        }));
        let (commands, receiver) = mpsc::sync_channel(1);
        let worker_state = state.clone();
        let worker_app = app.clone();
        std::thread::Builder::new()
            .name("waypin-wifi".into())
            .stack_size(24 * 1024)
            .spawn(move || run(wifi, receiver, worker_state, worker_app))?;
        let control = Self {
            state,
            commands,
            app,
        };
        if let Some(saved) = saved {
            control.submit(Command::Connect(saved))?;
        }
        Ok(control)
    }

    pub fn has_hotspot_client(&self) -> Option<bool> {
        let state = self.state.try_lock().ok()?;
        Some(
            state.live["client_count"]
                .as_i64()
                .is_some_and(|count| count > 0),
        )
    }

    pub fn snapshot(&self) -> Result<Value> {
        let state = self.state.lock().map_err(|_| "Wi-Fi 状态暂不可用")?;
        let mut value = state.live.clone();
        value["networks"] = json!(state.networks);
        value["result"] = json!(state.result);
        value["operation"] = state.operation.as_ref().map(|operation| json!({
            "id": operation.id, "kind": operation.kind, "elapsed_ms": operation.started.elapsed().as_millis() as u64
        })).unwrap_or(Value::Null);
        Ok(value)
    }

    pub fn submit(&self, command: Command) -> Result<u32> {
        self.app.begin_network_change()?;
        let result = (|| {
            let mut state = self.state.lock().map_err(|_| "Wi-Fi 状态暂不可用")?;
            let id = state.next_id;
            state.next_id += 1;
            let kind = command.name();
            self.commands
                .try_send((id, command))
                .map_err(|_| "Wi-Fi 操作队列繁忙，请重试")?;
            state.operation = Some(Operation {
                id,
                kind,
                started: Instant::now(),
            });
            Ok(id)
        })();
        if result.is_err() {
            self.app.end_network_change();
        }
        result
    }
}

fn run(
    mut wifi: WifiAccessPoint,
    receiver: mpsc::Receiver<(u32, Command)>,
    state: Arc<Mutex<State>>,
    app: Arc<App>,
) {
    loop {
        match receiver.recv_timeout(Duration::from_secs(1)) {
            Ok((id, command)) => {
                let kind = command.name();
                app.event(
                    "info",
                    match kind {
                        "scan" => "开始扫描 Wi-Fi",
                        "connect" => "开始连接上游 Wi-Fi",
                        "hotspot" => "开始修改盒子热点",
                        _ => "开始断开上游 Wi-Fi",
                    },
                );
                let result = match command {
                    Command::Scan => wifi.scan(),
                    Command::Connect(value) => wifi.connect_upstream(value).map(|_| Vec::new()),
                    Command::Disconnect => wifi.disconnect_upstream().map(|_| Vec::new()),
                    Command::Hotspot(value) => {
                        // Allow the accepted HTTP response to reach the phone
                        // before the AP deauthenticates its current clients.
                        std::thread::sleep(Duration::from_millis(1500));
                        wifi.update_hotspot(value).map(|_| Vec::new())
                    }
                };
                let ok = result.is_ok();
                let message = match &result {
                    Ok(rows) => match kind {
                        "scan" => format!("扫描完成，找到 {} 个网络", rows.len()),
                        "connect" => "盒子已连接上游 Wi-Fi，手机继续通过蜂窝数据上网".into(),
                        "hotspot" => "盒子热点已保存，请使用新的热点设置重新连接".into(),
                        _ => "已断开并忘记上游 Wi-Fi，盒子热点继续可用".into(),
                    },
                    Err(error) => explain_error(&error.to_string(), kind),
                };
                {
                    let mut state = state.lock().unwrap();
                    if let Ok(rows) = result {
                        if kind == "scan" {
                            state.networks = rows;
                        }
                    }
                    state.live = wifi.snapshot();
                    state.operation = None;
                    state.result =
                        Some(json!({"id": id, "kind": kind, "ok": ok, "message": message}));
                }
                app.event(if ok { "info" } else { "error" }, &message);
                app.end_network_change();
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let live = wifi.snapshot();
                let mut state = state.lock().unwrap();
                state.live = live;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
}

fn explain_error(detail: &str, kind: &str) -> String {
    if detail.starts_with("目标 Wi-Fi")
        || detail.starts_with("请")
        || detail.starts_with("Wi-Fi")
        || detail.starts_with("热点")
    {
        return detail.to_owned();
    }
    if kind == "hotspot" {
        return "热点修改失败，请重新连接当前热点后重试".into();
    }
    if detail.contains("TIMEOUT") {
        return "连接 Wi-Fi 超时，请检查密码和2.4GHz信号后重试".into();
    }
    "Wi-Fi 操作失败，请检查网络名称、密码及2.4GHz网络是否可用后重试".into()
}
