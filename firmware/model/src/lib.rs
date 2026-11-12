use serde::{Deserialize, Serialize};

pub mod backup;
pub mod button;
pub mod indicator;
pub mod maps;
pub use maps::{MapChange, MapConfig, MapCredentials, MapProvider, MapRegion};

use std::net::{IpAddr, Ipv4Addr};

pub const HOTSPOT_IP: Ipv4Addr = Ipv4Addr::new(192, 168, 71, 1);
pub const CONTROL_HOST: &str = "mappin.local";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControlLink {
    Wifi,
    Bluetooth,
}

pub fn control_link(
    local: IpAddr,
    peer: IpAddr,
    pan: Option<(Ipv4Addr, Ipv4Addr)>,
) -> Option<ControlLink> {
    let (local, peer) = (local.to_canonical(), peer.to_canonical());
    if pan.is_some_and(|(host, phone)| local == IpAddr::V4(host) && peer == IpAddr::V4(phone)) {
        return Some(ControlLink::Bluetooth);
    }
    (local == IpAddr::V4(HOTSPOT_IP) && peer != local && control_client(peer))
        .then_some(ControlLink::Wifi)
}

pub fn control_origin(local: IpAddr, host: &str, origin: Option<&str>) -> bool {
    let host = host.strip_suffix(":80").unwrap_or(host);
    let domain = host.strip_suffix('.').unwrap_or(host);
    if !domain.eq_ignore_ascii_case(CONTROL_HOST) && host != local.to_string() {
        return false;
    }
    origin.is_none_or(|origin| {
        origin
            .strip_prefix("http://")
            .is_some_and(|origin_host| origin_host.eq_ignore_ascii_case(host))
    })
}

/// Keep control on the accessory AP when the upstream station is enabled.
pub fn control_client(address: std::net::IpAddr) -> bool {
    match address.to_canonical() {
        std::net::IpAddr::V4(address) => {
            let bytes = address.octets();
            bytes[..3] == [192, 168, 71] && (1..=254).contains(&bytes[3])
        }
        _ => false,
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct WifiCredentials {
    pub ssid: String,
    pub password: String,
}

impl WifiCredentials {
    pub fn validate(self) -> Result<Self, String> {
        if self.ssid.is_empty() || self.ssid.len() > 32 || self.ssid.chars().any(char::is_control) {
            return Err("Wi-Fi 名称需要1至32字节，不能包含控制字符".into());
        }
        if (!self.password.is_empty() && !(8..=63).contains(&self.password.len()))
            || self.password.chars().any(char::is_control)
        {
            return Err("Wi-Fi 密码需要8至63字节，开放网络请留空".into());
        }
        Ok(self)
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct HotspotConfig {
    pub ssid: String,
    pub password: String,
}

#[derive(Clone, Deserialize)]
pub struct HotspotChange {
    pub ssid: String,
    pub password: Option<String>,
}

impl HotspotChange {
    pub fn validate(mut self) -> Result<Self, String> {
        if self.ssid.trim().is_empty()
            || self.ssid.len() > 32
            || self.ssid.chars().any(char::is_control)
        {
            return Err("热点名称需要1至32字节，不能全为空格或包含控制字符".into());
        }
        if self.password.as_deref() == Some("") {
            self.password = None;
        }
        if let Some(password) = &self.password {
            if !(8..=63).contains(&password.len())
                || !password.bytes().all(|byte| (32..=126).contains(&byte))
            {
                return Err("热点密码需要8至63位英文、数字或符号，留空保持原密码".into());
            }
        }
        Ok(self)
    }

    pub fn resolve(self, current: &HotspotConfig) -> Result<HotspotConfig, String> {
        let change = self.validate()?;
        Ok(HotspotConfig {
            ssid: change.ssid,
            password: change.password.unwrap_or_else(|| current.password.clone()),
        })
    }
}

impl HotspotConfig {
    pub fn validate(self) -> Result<Self, String> {
        if self.password.is_empty() {
            return Err("热点不能使用空密码".into());
        }
        HotspotChange {
            ssid: self.ssid.clone(),
            password: Some(self.password.clone()),
        }
        .validate()?;
        Ok(self)
    }
}

pub const MAX_FAVORITES: usize = 20;
pub const MAX_RECENT: usize = 6;
pub const MAX_NAME: usize = 64;
pub const MAX_ADDRESS: usize = 96;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Place {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latitude: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub longitude: Option<f64>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub address: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub google_place_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub query: String,
}

impl Place {
    pub fn validate(mut self) -> Result<Self, String> {
        self.name = self.name.trim().to_owned();
        if self.name.is_empty() {
            return Err("请输入地点名称".into());
        }
        if self.name.chars().count() > MAX_NAME || self.name.chars().any(char::is_control) {
            return Err(format!("地点名称最多 {MAX_NAME} 个字，不能包含换行"));
        }
        self.address = self.address.trim().to_owned();
        self.source = self.source.trim().to_owned();
        if self.address.chars().count() > MAX_ADDRESS || self.address.chars().any(char::is_control)
        {
            return Err(format!("地点地址最多 {MAX_ADDRESS} 个字，不能包含换行"));
        }
        if self.source.len() > 32 || self.source.chars().any(char::is_control) {
            return Err("地点来源格式无效".into());
        }
        if self.source == "google" {
            if self.google_place_id.is_empty()
                || self.google_place_id.len() > 256
                || !self
                    .google_place_id
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
                || self.query.trim().is_empty()
                || self.query.chars().count() > MAX_NAME
                || self.query.chars().any(char::is_control)
            {
                return Err("Google 地点引用无效，请重新搜索".into());
            }
            self.query = self.query.trim().into();
        } else if !self.google_place_id.is_empty() || !self.query.is_empty() {
            return Err("地点来源与引用不匹配".into());
        }
        match (self.latitude, self.longitude) {
            (None, None) if self.source == "google" => {}
            (Some(lat), Some(lng))
                if lat.is_finite()
                    && lng.is_finite()
                    && (-90.0..=90.0).contains(&lat)
                    && (-180.0..=180.0).contains(&lng) =>
            {
                self.latitude = Some((lat * 10_000_000.0).round() / 10_000_000.0);
                self.longitude = Some((lng * 10_000_000.0).round() / 10_000_000.0);
            }
            _ => return Err("地点坐标无效，请重新选择地点".into()),
        }
        Ok(self)
    }

    pub fn point(&self) -> Result<(f64, f64), String> {
        self.latitude
            .zip(self.longitude)
            .ok_or_else(|| "此地点需要联网解析，请在网页重新选择".into())
    }

    /// A Google favorite is a place ID and a user-supplied label, never a permanent POI cache.
    pub fn stored(self) -> Result<Self, String> {
        let mut place = self.validate()?;
        if place.source == "google" {
            place.latitude = None;
            place.longitude = None;
            place.address.clear();
            place.name = place.query.clone();
        }
        Ok(place)
    }

    pub fn same_position(&self, other: &Self) -> bool {
        if self.source == "google" || other.source == "google" {
            return self.source == other.source
                && !self.google_place_id.is_empty()
                && self.google_place_id == other.google_place_id;
        }
        self.latitude.is_some()
            && self.longitude.is_some()
            && self.latitude == other.latitude
            && self.longitude == other.longitude
    }
}

#[derive(Default)]
pub struct ReplayCache {
    google: Option<(u64, Place)>,
}

impl ReplayCache {
    pub fn remember(&mut self, place: &Place, now: u64) {
        self.google = (place.source == "google").then(|| (now + 600, place.clone()));
    }

    pub fn expire(&mut self, now: u64) {
        if self.google.as_ref().is_some_and(|(until, _)| now >= *until) {
            self.google = None;
        }
    }

    pub fn resolve(&mut self, place: Place, now: u64) -> Result<Place, String> {
        self.expire(now);
        if place.source != "google" {
            place.point()?;
            return Ok(place);
        }
        self.google
            .as_ref()
            .filter(|(_, cached)| cached.same_position(&place))
            .map(|(_, cached)| cached.clone())
            .ok_or_else(|| "Google 地点需要联网更新，请在网页重新定位该地点".into())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Favorite {
    pub id: u32,
    #[serde(flatten)]
    pub place: Place,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Favorites {
    pub version: u32,
    pub next_id: u32,
    pub items: Vec<Favorite>,
}

impl Default for Favorites {
    fn default() -> Self {
        Self {
            version: 2,
            next_id: 1,
            items: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Backup {
    pub version: u32,
    pub favorites: Vec<Place>,
}

impl Favorites {
    pub fn validate(&self) -> Result<(), String> {
        if ![1, 2].contains(&self.version) || self.next_id == 0 || self.items.len() > MAX_FAVORITES
        {
            return Err("收藏数据版本或数量无效".into());
        }
        for (index, favorite) in self.items.iter().enumerate() {
            favorite.place.clone().validate()?;
            if favorite.id == 0 || favorite.id >= self.next_id {
                return Err("收藏编号无效".into());
            }
            if self.items[..index]
                .iter()
                .any(|item| item.id == favorite.id || item.place.same_position(&favorite.place))
            {
                return Err("收藏数据包含重复地点".into());
            }
        }
        Ok(())
    }

    pub fn put(&mut self, id: Option<u32>, place: Place) -> Result<u32, String> {
        let place = place.stored()?;
        self.version = 2;
        if let Some(existing) = self
            .items
            .iter()
            .find(|item| Some(item.id) != id && item.place.same_position(&place))
        {
            return Err(format!("这个地点已收藏为“{}”", existing.place.name));
        }
        if let Some(id) = id {
            let item = self
                .items
                .iter_mut()
                .find(|item| item.id == id)
                .ok_or("这条收藏已经被删除，请刷新页面")?;
            item.place = place;
            return Ok(id);
        }
        if self.items.len() >= MAX_FAVORITES {
            return Err(format!(
                "最多保存 {MAX_FAVORITES} 个收藏，请先删除不需要的地点"
            ));
        }
        let id = self.next_id;
        self.next_id = id.checked_add(1).ok_or("收藏编号已用尽")?;
        self.items.push(Favorite { id, place });
        Ok(id)
    }

    pub fn remove(&mut self, id: u32) -> Result<(), String> {
        let index = self
            .items
            .iter()
            .position(|item| item.id == id)
            .ok_or("这条收藏已经被删除，请刷新页面")?;
        self.items.remove(index);
        Ok(())
    }

    /// Merge into a copy so an invalid or oversized import changes nothing.
    pub fn merge(&mut self, backup: Backup) -> Result<(usize, usize), String> {
        if ![1, 2].contains(&backup.version) || backup.favorites.len() > MAX_FAVORITES {
            return Err("请选择盒子导出的收藏备份，最多包含 20 个地点".into());
        }
        let places = backup
            .favorites
            .into_iter()
            .map(Place::stored)
            .collect::<Result<Vec<_>, _>>()?;
        let mut next = self.clone();
        let mut added = 0;
        let mut skipped = 0;
        for place in places {
            if next
                .items
                .iter()
                .any(|item| item.place.same_position(&place))
            {
                skipped += 1;
                continue;
            }
            next.put(None, place)?;
            added += 1;
        }
        *self = next;
        Ok((added, skipped))
    }

    pub fn backup(&self) -> Backup {
        Backup {
            version: 2,
            favorites: self.items.iter().map(|item| item.place.clone()).collect(),
        }
    }
}

pub fn record_recent(history: &mut Vec<Place>, place: Place) {
    let Ok(place) = place.stored() else {
        return;
    };
    history.retain(|item| !item.same_position(&place));
    history.insert(0, place);
    history.truncate(MAX_RECENT);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_api_accepts_hotspot_clients_and_rejects_upstream_clients() {
        for address in ["192.168.71.2", "192.168.71.254", "::ffff:192.168.71.2"] {
            assert!(control_client(address.parse().unwrap()));
        }
        for address in [
            "192.168.100.2",
            "192.168.70.2",
            "192.168.71.0",
            "192.168.71.255",
            "127.0.0.1",
            "203.0.113.1",
            "::1",
            "fe80::1",
        ] {
            assert!(!control_client(address.parse().unwrap()));
        }
    }

    #[test]
    fn control_requires_the_matching_local_interface_and_pan_gateway() {
        let pan = Some((
            "172.20.10.3".parse().unwrap(),
            "172.20.10.1".parse().unwrap(),
        ));
        let link = |local: &str, peer: &str| {
            control_link(local.parse().unwrap(), peer.parse().unwrap(), pan)
        };
        assert_eq!(
            link("::ffff:172.20.10.3", "::ffff:172.20.10.1"),
            Some(ControlLink::Bluetooth)
        );
        assert_eq!(
            link("192.168.71.1", "192.168.71.20"),
            Some(ControlLink::Wifi)
        );
        assert_eq!(link("192.168.71.88", "192.168.71.20"), None);
        assert_eq!(link("172.20.10.3", "172.20.10.4"), None);
        assert_eq!(link("172.20.10.3", "192.168.71.20"), None);
        assert_eq!(link("192.168.71.1", "192.168.71.1"), None);
        assert_eq!(
            control_link(
                "172.20.10.3".parse().unwrap(),
                "172.20.10.1".parse().unwrap(),
                None
            ),
            None
        );
    }

    #[test]
    fn control_origin_accepts_the_local_name_and_the_receiving_address() {
        for local in [HOTSPOT_IP, Ipv4Addr::new(172, 20, 10, 3)] {
            let address = IpAddr::V4(local);
            let host = local.to_string();
            assert!(control_origin(address, &host, None));
            assert!(control_origin(
                address,
                &format!("{host}:80"),
                Some(&format!("http://{host}"))
            ));
            assert!(control_origin(
                address,
                "MAPPIN.LOCAL:80",
                Some("http://mappin.local")
            ));
            assert!(!control_origin(
                address,
                "mappin.local",
                Some(&format!("http://{host}"))
            ));
            assert!(!control_origin(
                address,
                "external.example",
                Some("http://external.example")
            ));
        }
    }

    #[test]
    fn wifi_credentials_measure_utf8_bytes_and_preserve_password_whitespace() {
        let value = WifiCredentials {
            ssid: "家".repeat(10),
            password: " pass word ".into(),
        }
        .validate()
        .unwrap();
        assert_eq!(value.password, " pass word ");
        assert!(
            WifiCredentials {
                ssid: "家".repeat(11),
                password: String::new()
            }
            .validate()
            .is_err()
        );
        assert!(
            WifiCredentials {
                ssid: "网络".into(),
                password: "short".into()
            }
            .validate()
            .is_err()
        );
        assert!(
            WifiCredentials {
                ssid: "网络\n".into(),
                password: String::new()
            }
            .validate()
            .is_err()
        );
        assert!(
            WifiCredentials {
                ssid: "Open network".into(),
                password: String::new()
            }
            .validate()
            .is_ok()
        );
    }

    #[test]
    fn hotspot_rename_preserves_the_password_for_null_or_empty_input() {
        let current = HotspotConfig {
            ssid: "MapPin".into(),
            password: " old pass ".into(),
        };
        for password in [None, Some(String::new())] {
            let next = HotspotChange {
                ssid: "我的盒子".into(),
                password,
            }
            .resolve(&current)
            .unwrap();
            assert_eq!(next.password, current.password);
            assert_eq!(next.ssid, "我的盒子");
        }
        let stored: HotspotConfig =
            serde_json::from_slice(&serde_json::to_vec(&current).unwrap()).unwrap();
        assert!(stored.validate().is_ok());
    }

    #[test]
    fn hotspot_requires_a_usable_name_and_a_wpa2_passphrase() {
        for ssid in [" ".into(), "家".repeat(11), "bad\nname".into()] {
            assert!(
                HotspotChange {
                    ssid,
                    password: None
                }
                .validate()
                .is_err()
            );
        }
        for password in [
            "1234567".into(),
            "a".repeat(64),
            "密码密码密码".into(),
            "hello\nworld".into(),
        ] {
            assert!(
                HotspotChange {
                    ssid: "盒子".into(),
                    password: Some(password)
                }
                .validate()
                .is_err()
            );
        }
        assert!(
            HotspotConfig {
                ssid: "盒子".into(),
                password: String::new()
            }
            .validate()
            .is_err()
        );
        let value = HotspotChange {
            ssid: "家".repeat(10),
            password: Some(" pass word ".into()),
        }
        .validate()
        .unwrap();
        assert_eq!(value.password.as_deref(), Some(" pass word "));
    }

    #[test]
    fn map_credentials_require_a_complete_pair_and_trim_pasted_whitespace() {
        let value = MapCredentials {
            key: format!("  {}\n", "k".repeat(32)),
            security_code: format!(" {} ", "s".repeat(32)),
        }
        .validate()
        .unwrap();
        assert!(value.configured());
        assert_eq!(value.key, "k".repeat(32));
        assert_eq!(value.security_code, "s".repeat(32));
        assert!(!MapCredentials::default().configured());
        for security_code in ["".to_owned(), "a".repeat(129), "a\nb".repeat(10)] {
            assert!(
                MapCredentials {
                    key: "k".repeat(32),
                    security_code
                }
                .validate()
                .is_err()
            );
        }
        assert!(serde_json::from_str::<MapCredentials>(r#"{"key":"incomplete"}"#).is_err());
    }

    fn place(name: &str, latitude: f64) -> Place {
        Place {
            name: name.into(),
            latitude: Some(latitude),
            longitude: Some(121.4737),
            ..Default::default()
        }
    }

    #[test]
    fn coordinates_must_be_finite_and_in_range() {
        for latitude in [f64::NAN, f64::INFINITY, -90.1, 90.1] {
            assert!(place("地点", latitude).validate().is_err());
        }
        let mut value = place("地点", 31.2304);
        value.longitude = Some(181.0);
        assert!(value.validate().is_err());
        assert!(place("极点", -90.0).validate().is_ok());
    }

    #[test]
    fn legacy_favorites_gain_optional_metadata_without_losing_coordinates() {
        let old = r#"{"name":"上海市中心","latitude":31.2304,"longitude":121.4737}"#;
        let value: Place = serde_json::from_str(old).unwrap();
        let value = value.validate().unwrap();
        assert_eq!(value.name, "上海市中心");
        assert_eq!(value.latitude, Some(31.2304));
        assert!(value.address.is_empty());
        assert!(value.source.is_empty());
    }

    #[test]
    fn searched_place_metadata_survives_backup_and_restore() {
        let mut value = place("上海虹桥站", 31.1961169);
        value.address = "上海市 闵行区 申贵路".into();
        value.source = "openstreetmap".into();
        let mut original = Favorites::default();
        original.put(None, value.clone()).unwrap();
        let backup = serde_json::to_vec(&original.backup()).unwrap();
        let mut restored = Favorites::default();
        restored
            .merge(serde_json::from_slice(&backup).unwrap())
            .unwrap();
        assert_eq!(restored.items[0].place, value);
        let mut bad = value;
        bad.address = "地址".repeat(MAX_ADDRESS);
        assert!(restored.put(None, bad).is_err());
        assert_eq!(restored.items.len(), 1);
    }

    #[test]
    fn maximum_place_metadata_fits_storage_and_request_limit() {
        let mut book = Favorites::default();
        for index in 0..MAX_FAVORITES {
            let mut value = place(&"𐀀".repeat(MAX_NAME), index as f64);
            value.address = "𐀀".repeat(MAX_ADDRESS);
            value.source = "x".repeat(32);
            book.put(None, value).unwrap();
        }
        assert!(serde_json::to_vec(&book).unwrap().len() <= 16384);
        assert!(serde_json::to_vec(&book.backup()).unwrap().len() <= 16384);
    }

    #[test]
    fn chinese_names_are_counted_as_characters() {
        assert!(place(&"沪".repeat(MAX_NAME), 31.0).validate().is_ok());
        assert!(place(&"沪".repeat(MAX_NAME + 1), 31.0).validate().is_err());
        assert!(place("\n", 31.0).validate().is_err());
        assert!(place("甲\u{1f}乙", 31.0).validate().is_err());
        assert_eq!(place(" 家 ", 31.0).validate().unwrap().name, "家");
    }

    #[test]
    fn editing_keeps_id_and_rejects_duplicate_coordinates() {
        let mut book = Favorites::default();
        let id = book.put(None, place("家", 31.0)).unwrap();
        book.put(Some(id), place("新家", 32.0)).unwrap();
        assert_eq!(book.items[0].id, id);
        assert_eq!(book.items[0].place.name, "新家");
        assert!(book.put(None, place("重复", 32.0)).is_err());
        book.remove(id).unwrap();
        assert!(book.items.is_empty());
        assert!(book.put(Some(id), place("已删", 33.0)).is_err());
    }

    #[test]
    fn failed_import_is_atomic() {
        let mut book = Favorites::default();
        book.put(None, place("原地点", 31.0)).unwrap();
        let original = serde_json::to_string(&book).unwrap();
        let backup = Backup {
            version: 1,
            favorites: vec![place("有效", 32.0), place("无效", 99.0)],
        };
        assert!(book.merge(backup).is_err());
        assert_eq!(serde_json::to_string(&book).unwrap(), original);
    }

    #[test]
    fn import_skips_duplicates_and_preserves_existing_names() {
        let mut book = Favorites::default();
        book.put(None, place("家", 31.0)).unwrap();
        let backup = Backup {
            version: 1,
            favorites: vec![place("别名", 31.0), place("公司", 32.0)],
        };
        assert_eq!(book.merge(backup).unwrap(), (1, 1));
        assert_eq!(book.items[0].place.name, "家");
        let encoded = serde_json::to_vec(&book.backup()).unwrap();
        let mut restored = Favorites::default();
        restored
            .merge(serde_json::from_slice(&encoded).unwrap())
            .unwrap();
        assert_eq!(restored.items.len(), 2);
        restored.validate().unwrap();
    }

    #[test]
    fn full_import_does_not_partially_append() {
        let mut book = Favorites::default();
        for index in 0..19 {
            book.put(None, place("地点", index as f64)).unwrap();
        }
        let backup = Backup {
            version: 1,
            favorites: vec![place("甲", 30.0), place("乙", 31.0)],
        };
        assert!(book.merge(backup).is_err());
        assert_eq!(book.items.len(), 19);
    }

    #[test]
    fn history_moves_reused_coordinates_to_the_front() {
        let mut history = Vec::new();
        for index in 0..10 {
            record_recent(&mut history, place("地点", index as f64));
        }
        assert_eq!(history.len(), MAX_RECENT);
        record_recent(&mut history, place("新名称", 7.0));
        assert_eq!(history.len(), MAX_RECENT);
        assert_eq!(history[0].name, "新名称");
        assert_eq!(
            history
                .iter()
                .filter(|item| item.latitude == Some(7.0))
                .count(),
            1
        );
    }

    #[test]
    fn unsupported_backup_version_is_rejected() {
        assert!(
            Favorites::default()
                .merge(Backup {
                    version: 99,
                    favorites: vec![]
                })
                .is_err()
        );
    }
    fn google() -> Place {
        Place {
            name: "API business name".into(),
            latitude: Some(31.2),
            longitude: Some(121.4),
            address: "API address".into(),
            source: "google".into(),
            google_place_id: "ChIJ_example_123".into(),
            query: "用户搜索词".into(),
        }
    }

    #[test]
    fn google_favorites_backups_and_history_only_keep_user_labels_and_ids() {
        let mut book = Favorites::default();
        book.put(None, google()).unwrap();
        let json = serde_json::to_string(&book.backup()).unwrap();
        for forbidden in [
            "latitude",
            "longitude",
            "API business",
            "API address",
            "address",
        ] {
            assert!(!json.contains(forbidden));
        }
        let mut restored = Favorites::default();
        restored
            .merge(serde_json::from_str(&json).unwrap())
            .unwrap();
        assert_eq!(restored.items[0].place.name, "用户搜索词");
        assert_eq!(restored.items[0].place.point().is_err(), true);
        let mut recent = vec![];
        record_recent(&mut recent, google());
        assert!(recent[0].latitude.is_none());
        assert!(book.put(None, google()).is_err());
    }

    #[test]
    fn replay_expiry_never_reuses_an_old_or_unrelated_google_point() {
        let place = google();
        let saved = place.clone().stored().unwrap();
        let mut cache = ReplayCache::default();
        assert!(cache.resolve(saved.clone(), 0).is_err());
        cache.remember(&place, 100);
        assert_eq!(
            cache.resolve(saved.clone(), 699).unwrap().point().unwrap(),
            (31.2, 121.4)
        );
        let mut other = saved.clone();
        other.google_place_id = "other_id".into();
        assert!(cache.resolve(other, 200).is_err());
        assert!(cache.resolve(saved, 700).is_err());
        assert!(cache.google.is_none());
        assert!(
            cache
                .resolve(super::tests::place("固定地点", 31.0), 99999)
                .is_ok()
        );
    }
}
