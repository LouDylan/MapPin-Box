use crate::{
    app::App,
    diagnostics,
    wifi_control::{Command, WifiControl},
};
use esp_idf_svc::{
    handle::RawHandle,
    http::{
        Method,
        server::{Configuration, EspHttpConnection, EspHttpServer, Request},
    },
    io::{Read, Write},
    sys,
};
use waypin_box_model::{
    Backup, HotspotChange, MapChange, Place, WifiCredentials,
    backup::{BackupExport, EXPORT_WINDOW},
    control_client,
};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::{
    error::Error,
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    sync::{Arc, Mutex},
    time::Instant,
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

#[derive(Debug)]
struct ControlAccessDenied;

impl std::fmt::Display for ControlAccessDenied {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("请连接盒子热点后访问控制与管理功能")
    }
}

impl Error for ControlAccessDenied {}

fn client_address(request: &mut Request<&mut EspHttpConnection<'_>>) -> Result<IpAddr> {
    let fd = unsafe { sys::httpd_req_to_sockfd(request.connection().handle()) };
    let mut storage = sys::sockaddr_storage::default();
    let mut size = size_of::<sys::sockaddr_storage>() as sys::socklen_t;
    let result = unsafe {
        sys::lwip_getpeername(fd, &mut storage as *mut _ as *mut sys::sockaddr, &mut size)
    };
    if result != 0 {
        return Err("无法读取手机网络地址".into());
    }
    // HTTPD may accept IPv4 clients through its IPv6 listening socket. Read
    // sockaddr_storage, then decode the actual family, including ::ffff:v4.
    let address = match u32::from(storage.ss_family) {
        sys::AF_INET => {
            let address = unsafe { &*(&storage as *const _ as *const sys::sockaddr_in) };
            IpAddr::V4(Ipv4Addr::from(address.sin_addr.s_addr.to_ne_bytes()))
        }
        sys::AF_INET6 => {
            let address = unsafe { &*(&storage as *const _ as *const sys::sockaddr_in6) };
            Ipv6Addr::from(unsafe { address.sin6_addr.un.u8_addr }).to_canonical()
        }
        _ => return Err("手机网络地址类型无效".into()),
    };
    if address.is_unspecified() || address.is_loopback() || address.is_multicast() {
        return Err("手机网络地址无效，请重新连接盒子 Wi-Fi".into());
    }
    if !control_client(address) {
        return Err(Box::new(ControlAccessDenied));
    }
    Ok(address)
}

fn respond(request: Request<&mut EspHttpConnection<'_>>, status: u16, value: Value) -> Result<()> {
    request
        .into_response(
            status,
            None,
            &[
                ("Content-Type", "application/json; charset=utf-8"),
                ("Cache-Control", "no-store"),
            ],
        )?
        .write_all(&serde_json::to_vec(&value)?)?;
    Ok(())
}

fn finish(
    request: Request<&mut EspHttpConnection<'_>>,
    result: Result<Value>,
    status: u16,
) -> Result<()> {
    match result {
        Ok(value) => respond(request, status, value),
        Err(error) => respond(
            request,
            if error.is::<ControlAccessDenied>() {
                403
            } else {
                400
            },
            json!({"ok": false, "message": error.to_string()}),
        ),
    }
}

fn read_json<T: DeserializeOwned>(request: &mut Request<&mut EspHttpConnection<'_>>) -> Result<T> {
    client_address(request)?;
    use embedded_svc::http::Headers;
    if !request
        .header("Content-Type")
        .unwrap_or_default()
        .starts_with("application/json")
    {
        return Err("请发送 JSON 格式的数据".into());
    }
    let size = request.content_len().ok_or("缺少请求长度")? as usize;
    if size == 0 || size > 16384 {
        return Err("请求过大，最多允许 16KB 数据".into());
    }
    let mut bytes = vec![0; size];
    request.read_exact(&mut bytes)?;
    serde_json::from_slice(&bytes).map_err(|_| "数据格式不正确，请检查输入内容".into())
}

#[derive(Deserialize)]
struct SaveFavorite {
    id: Option<u32>,
    #[serde(flatten)]
    place: Place,
}

#[derive(Deserialize)]
struct RecordId {
    id: u32,
}

#[derive(Deserialize)]
struct Empty {}

#[derive(Deserialize)]
struct ExportRequest {
    token: Option<String>,
}

enum ExportReply {
    Waiting(Value),
    File(Vec<u8>),
}

pub fn start(
    app: Arc<App>,
    wifi: WifiControl,
    backup: Arc<Mutex<BackupExport>>,
) -> Result<EspHttpServer<'static>> {
    let mut server = EspHttpServer::new(&Configuration {
        stack_size: 16 * 1024,
        max_open_sockets: 4,
        max_uri_handlers: 64,
        ..Default::default()
    })?;
    for (path, mime, bytes) in [
        (
            "/google-sdk.mjs",
            "text/javascript; charset=utf-8",
            include_bytes!("google-sdk.mjs").as_slice(),
        ),
        (
            "/current-location.mjs",
            "text/javascript; charset=utf-8",
            include_bytes!("current-location.mjs").as_slice(),
        ),
        (
            "/hotspot.mjs",
            "text/javascript; charset=utf-8",
            include_bytes!("hotspot.mjs").as_slice(),
        ),
        (
            "/landmarks.mjs",
            "text/javascript; charset=utf-8",
            include_bytes!("landmarks.mjs").as_slice(),
        ),
        (
            "/",
            "text/html; charset=utf-8",
            include_bytes!("portal.html").as_slice(),
        ),
        (
            "/admin.js",
            "text/javascript; charset=utf-8",
            include_bytes!("admin.js").as_slice(),
        ),
        (
            "/admin.css",
            "text/css; charset=utf-8",
            include_bytes!("admin.css").as_slice(),
        ),
        (
            "/api.mjs",
            "text/javascript; charset=utf-8",
            include_bytes!("api.mjs").as_slice(),
        ),
        (
            "/app.css",
            "text/css; charset=utf-8",
            include_bytes!("portal.css").as_slice(),
        ),
        (
            "/app.js",
            "text/javascript; charset=utf-8",
            include_bytes!("portal.js").as_slice(),
        ),
        (
            "/coordinates.mjs",
            "text/javascript; charset=utf-8",
            include_bytes!("coordinates.mjs").as_slice(),
        ),
        (
            "/places.mjs",
            "text/javascript; charset=utf-8",
            include_bytes!("places.mjs").as_slice(),
        ),
        (
            "/map-service.mjs",
            "text/javascript; charset=utf-8",
            include_bytes!("map-service.mjs").as_slice(),
        ),
        (
            "/place-picker.mjs",
            "text/javascript; charset=utf-8",
            include_bytes!("place-picker.mjs").as_slice(),
        ),
        (
            "/map-view.mjs",
            "text/javascript; charset=utf-8",
            include_bytes!("map-view.mjs").as_slice(),
        ),
        (
            "/assets/maplibre-gl.mjs",
            "text/javascript; charset=utf-8",
            include_bytes!("assets/maplibre-gl.mjs").as_slice(),
        ),
        (
            "/assets/maplibre-gl-worker.mjs",
            "text/javascript; charset=utf-8",
            include_bytes!("assets/maplibre-gl-worker.mjs").as_slice(),
        ),
        (
            "/assets/maplibre-gl-shared.mjs",
            "text/javascript; charset=utf-8",
            include_bytes!("assets/maplibre-gl-shared.mjs").as_slice(),
        ),
        (
            "/assets/maplibre-gl.css",
            "text/css; charset=utf-8",
            include_bytes!("assets/maplibre-gl.css").as_slice(),
        ),
        (
            "/map-coordinates.mjs",
            "text/javascript; charset=utf-8",
            include_bytes!("map-coordinates.mjs").as_slice(),
        ),
        (
            "/coord-transform.mjs",
            "text/javascript; charset=utf-8",
            include_bytes!("coord-transform.mjs").as_slice(),
        ),
    ] {
        server.fn_handler(path, Method::Get, move |request| -> Result<()> {
            request
                .into_response(
                    200,
                    None,
                    &[("Content-Type", mime), ("Cache-Control", "no-store")],
                )?
                .write_all(bytes)?;
            Ok(())
        })?;
    }
    for (path, mime, bytes) in [
        (
            "/lucide-icons.mjs",
            "text/javascript; charset=utf-8",
            include_bytes!("lucide-icons.mjs").as_slice(),
        ),
        (
            "/favicon.svg",
            "image/svg+xml",
            include_bytes!("assets/favicon.svg").as_slice(),
        ),
    ] {
        server.fn_handler(path, Method::Get, move |request| -> Result<()> {
            request
                .into_response(
                    200,
                    None,
                    &[("Content-Type", mime), ("Cache-Control", "no-store")],
                )?
                .write_all(bytes)?;
            Ok(())
        })?;
    }
    server.fn_handler("/licenses.txt", Method::Get, |request| -> Result<()> {
        request
            .into_response(200, None, &[("Content-Type", "text/plain; charset=utf-8")])?
            .write_all(include_bytes!("assets/licenses.txt"))?;
        Ok(())
    })?;
    for (path, target) in [
        ("/connect", "/admin/help#iphone"),
        ("/admin/backup", "/admin/maps#backup"),
        ("/admin/events", "/admin/location#events"),
    ] {
        server.fn_handler(path, Method::Get, move |request| -> Result<()> {
            request
                .into_response(
                    302,
                    None,
                    &[("Location", target), ("Cache-Control", "no-store")],
                )?
                .write_all(b"")?;
            Ok(())
        })?;
    }
    for &(path, page, title, content) in crate::admin_pages::PAGES {
        server.fn_handler(path, Method::Get, move |request| -> Result<()> {
            let html = crate::admin_pages::render(page, title, content);
            let mut response = request.into_response(
                200,
                Some("OK"),
                &[
                    ("Content-Type", "text/html; charset=utf-8"),
                    ("Cache-Control", "no-store"),
                ],
            )?;
            response.write_all(html.as_bytes())?;
            Ok(())
        })?;
    }
    let hotspot_wifi = wifi.clone();
    server.fn_handler(
        "/api/wifi/hotspot",
        Method::Post,
        move |mut request| -> Result<()> {
            let result = (|| {
                let value = read_json::<HotspotChange>(&mut request)?.validate()?;
                let ssid = value.ssid.clone();
                let id = hotspot_wifi.submit(Command::Hotspot(value))?;
                Ok(json!({"ok": true, "job_id": id, "ssid": ssid,
                      "message": "热点修改已提交，请稍后使用新设置重新连接"}))
            })();
            finish(request, result, 202)
        },
    )?;
    let state_app = app.clone();
    let admin_app = app.clone();
    let admin_wifi = wifi.clone();
    server.fn_handler(
        "/api/admin/status",
        Method::Get,
        move |mut request| -> Result<()> {
            let result = (|| {
                client_address(&mut request)?;
                Ok(
                    json!({"device": diagnostics::snapshot(), "app": admin_app.snapshot()?,
            "wifi": admin_wifi.snapshot()?, "debug": admin_app.debug_state()?}),
                )
            })();
            finish(request, result, 200)
        },
    )?;
    let export_app = app.clone();
    server.fn_handler(
        "/api/backup/export",
        Method::Post,
        move |mut request| -> Result<()> {
            let result = (|| -> Result<ExportReply> {
                if !matches!(
                    request.header("Host"),
                    Some("192.168.71.1" | "192.168.71.1:80")
                ) || request
                    .header("Origin")
                    .is_some_and(|origin| origin != "http://192.168.71.1")
                {
                    return Err(Box::new(ControlAccessDenied));
                }
                let client = client_address(&mut request)?;
                let body = read_json::<ExportRequest>(&mut request)?;
                let mut approval = backup.lock().map_err(|_| "备份功能暂不可用")?;
                let now = Instant::now();
                let Some(token) = body.token else {
                    let token = unsafe {
                        format!(
                            "{:08x}{:08x}{:08x}{:08x}",
                            sys::esp_random(),
                            sys::esp_random(),
                            sys::esp_random(),
                            sys::esp_random()
                        )
                    };
                    approval.begin(client, token.clone(), now)?;
                    return Ok(ExportReply::Waiting(
                        json!({"token": token, "expires_in": EXPORT_WINDOW.as_secs()}),
                    ));
                };
                if !approval.claim(client, &token, now)? {
                    return Ok(ExportReply::Waiting(json!({"waiting": true})));
                }
                drop(approval);
                Ok(ExportReply::File(export_app.export_backup()?))
            })();
            match result {
                Ok(ExportReply::Waiting(value)) => respond(request, 202, value),
                Ok(ExportReply::File(file)) => {
                    request
                        .into_response(
                            200,
                            None,
                            &[
                                ("Content-Type", "application/octet-stream"),
                                ("Cache-Control", "no-store"),
                                ("Content-Disposition", "attachment; filename=waypin-box.waypinbackup"),
                                ("X-Content-Type-Options", "nosniff"),
                            ],
                        )?
                        .write_all(&file)?;
                    Ok(())
                }
                Err(error) => finish(request, Err(error), 200),
            }
        },
    )?;
    for (path, scanning) in [("/api/wifi/scan", true), ("/api/wifi/disconnect", false)] {
        let network = wifi.clone();
        server.fn_handler(path, Method::Post, move |mut request| -> Result<()> {
            let result = (|| {
                read_json::<Empty>(&mut request)?;
                let id = network.submit(if scanning {
                    Command::Scan
                } else {
                    Command::Disconnect
                })?;
                Ok(json!({"ok": true, "job_id": id}))
            })();
            finish(request, result, 202)
        })?;
    }
    server.fn_handler(
        "/api/wifi/connect",
        Method::Post,
        move |mut request| -> Result<()> {
            let result = (|| {
                let value = read_json::<WifiCredentials>(&mut request)?.validate()?;
                let id = wifi.submit(Command::Connect(value))?;
                Ok(json!({"ok": true, "job_id": id}))
            })();
            finish(request, result, 202)
        },
    )?;
    let replay = app.clone();
    server.fn_handler(
        "/api/replay",
        Method::Post,
        move |mut request| -> Result<()> {
            let result = (|| {
                read_json::<Empty>(&mut request)?;
                Ok(json!({"ok": true, "job_id": replay.replay_last()?}))
            })();
            finish(request, result, 202)
        },
    )?;
    let logs = app.clone();
    server.fn_handler(
        "/api/events/clear",
        Method::Post,
        move |mut request| -> Result<()> {
            let result = (|| {
                read_json::<Empty>(&mut request)?;
                logs.clear_events();
                Ok(json!({"ok": true}))
            })();
            finish(request, result, 200)
        },
    )?;
    let map_read = app.clone();
    server.fn_handler(
        "/api/maps/config",
        Method::Get,
        move |mut request| -> Result<()> {
            let result = (|| {
                client_address(&mut request)?;
                Ok(map_read.map_config()?.public())
            })();
            finish(request, result, 200)
        },
    )?;
    let map_write = app.clone();
    server.fn_handler(
        "/api/maps/config",
        Method::Post,
        move |mut request| -> Result<()> {
            let result = (|| {
                map_write.save_map_config(read_json::<MapChange>(&mut request)?)?;
                Ok(json!({"ok": true, "message": "地图服务配置已保存"}))
            })();
            finish(request, result, 200)
        },
    )?;
    server.fn_handler(
        "/api/status",
        Method::Get,
        move |mut request| -> Result<()> {
            let result = client_address(&mut request).and_then(|_| state_app.snapshot());
            finish(request, result, 200)
        },
    )?;
    let set_app = app.clone();
    server.fn_handler("/api/set", Method::Post, move |mut request| -> Result<()> {
        let result = (|| {
            let address = client_address(&mut request)?;
            let place = read_json::<Place>(&mut request)?;
            let id = set_app.set(address, place)?;
            Ok(json!({"ok": true, "job_id": id, "message": "定位操作已提交"}))
        })();
        finish(request, result, 202)
    })?;
    let restore_app = app.clone();
    server.fn_handler(
        "/api/restore",
        Method::Post,
        move |mut request| -> Result<()> {
            let result = (|| {
                let address = client_address(&mut request)?;
                read_json::<Empty>(&mut request)?;
                let id = restore_app.restore(Some(address))?;
                Ok(json!({"ok": true, "job_id": id, "message": "恢复操作已提交"}))
            })();
            finish(request, result, 202)
        },
    )?;
    let save_app = app.clone();
    server.fn_handler(
        "/api/favorites",
        Method::Post,
        move |mut request| -> Result<()> {
            let result = (|| {
                let value = read_json::<SaveFavorite>(&mut request)?;
                let id = save_app.save(value.id, value.place)?;
                Ok(json!({"ok": true, "id": id, "message": "收藏已保存"}))
            })();
            finish(request, result, 200)
        },
    )?;
    let delete_app = app.clone();
    server.fn_handler(
        "/api/favorites/delete",
        Method::Post,
        move |mut request| -> Result<()> {
            let result = (|| {
                delete_app.delete(read_json::<RecordId>(&mut request)?.id)?;
                Ok(json!({"ok": true, "message": "已删除收藏"}))
            })();
            finish(request, result, 200)
        },
    )?;
    let import_app = app.clone();
    server.fn_handler("/api/favorites/import", Method::Post, move |mut request| -> Result<()> {
        let result = (|| {
            let (added, skipped) = import_app.import(read_json::<Backup>(&mut request)?)?;
            Ok(json!({"ok": true, "message": format!("已导入 {added} 个地点，跳过 {skipped} 个重复坐标")}))
        })();
        finish(request, result, 200)
    })?;
    server.fn_handler(
        "/api/history/clear",
        Method::Post,
        move |mut request| -> Result<()> {
            let result = (|| {
                read_json::<Empty>(&mut request)?;
                app.clear_recent()?;
                Ok(json!({"ok": true, "message": "最近使用记录已清空"}))
            })();
            finish(request, result, 200)
        },
    )?;
    Ok(server)
}
