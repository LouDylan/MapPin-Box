//! Local accessory AP with an optional upstream station connection.
use crate::persistent_storage::PersistentNvsPartition;
use esp_idf_svc::{
    eventloop::{EspSystemEventLoop, EspSystemSubscription},
    hal::modem::Modem,
    handle::RawHandle,
    ipv4,
    netif::{EspNetif, IpEvent, NetifConfiguration, NetifStack},
    nvs::{EspKeyValueStorage, EspNvs, NvsCustom},
    sys::{self, esp},
    wifi::{
        AccessPointConfiguration, AuthMethod, BlockingWifi, ClientConfiguration, Configuration,
        EspWifi, WifiDeviceId, WifiDriver,
    },
};
use waypin_box_model::{HotspotChange, HotspotConfig, WifiCredentials};
use serde_json::{Value, json};
use std::{error::Error, net::Ipv4Addr};

type Result<T> = std::result::Result<T, Box<dyn Error>>;
pub const PASSWORD: &str = "waypin-setup";
const CHANNEL: u8 = 6;

pub struct WifiAccessPoint {
    wifi: BlockingWifi<EspWifi<'static>>,
    _ip_subscription: EspSystemSubscription<'static>,
    storage: EspKeyValueStorage<NvsCustom>,
    ap_storage: EspKeyValueStorage<NvsCustom>,
    ap_configuration: AccessPointConfiguration,
    saved: Option<WifiCredentials>,
    address: Ipv4Addr,
    subnet: ipv4::Subnet,
}

impl WifiAccessPoint {
    pub fn start(modem: Modem<'static>, partition: PersistentNvsPartition) -> Result<Self> {
        let event_loop = EspSystemEventLoop::take()?;
        let driver = WifiDriver::new(modem, event_loop.clone(), None)?;
        let mut router = ipv4::RouterConfiguration::default();
        router.dns = None;
        router.secondary_dns = None;
        let ap_netif = EspNetif::new_with_conf(&NetifConfiguration {
            ip_configuration: Some(ipv4::Configuration::Router(router)),
            ..NetifConfiguration::wifi_default_router()
        })?;
        local_dhcp(
            &ap_netif,
            sys::esp_netif_dhcp_option_mode_t_ESP_NETIF_OP_SET,
        )?;
        let mut wifi = BlockingWifi::wrap(
            EspWifi::wrap_all(driver, EspNetif::new(NetifStack::Sta)?, ap_netif)?,
            event_loop.clone(),
        )?;
        let mac = wifi.wifi().get_mac(WifiDeviceId::Ap)?;
        let ap_storage = EspKeyValueStorage::new(EspNvs::new(partition.clone(), "hotspot", true)?);
        let mut ap_buffer = [0_u8; 512];
        let hotspot = match ap_storage.get_raw("config_v1", &mut ap_buffer)? {
            Some(bytes) => serde_json::from_slice::<HotspotConfig>(bytes)?.validate()?,
            None => HotspotConfig {
                ssid: format!("Waypin-{:02X}{:02X}", mac[4], mac[5]),
                password: PASSWORD.into(),
            },
        };
        let ap_configuration = AccessPointConfiguration {
            ssid: hotspot.ssid.as_str().try_into()?,
            ssid_hidden: false,
            channel: CHANNEL,
            auth_method: AuthMethod::WPA2Personal,
            password: hotspot.password.as_str().try_into()?,
            max_connections: 4,
            ..Default::default()
        };
        let ip_subscription = event_loop.subscribe::<IpEvent, _>(|event| {
            if let IpEvent::ApStaIpAssigned(assignment) = event {
                log::info!("Wi-Fi: client received address {}", assignment.ip());
            }
        })?;
        wifi.set_configuration(&Configuration::AccessPoint(ap_configuration.clone()))?;
        wifi.start()?;
        wifi.wait_netif_up()?;
        local_dhcp(
            wifi.wifi().ap_netif(),
            sys::esp_netif_dhcp_option_mode_t_ESP_NETIF_OP_GET,
        )?;
        let info = wifi.wifi().ap_netif().get_ip_info()?;
        let storage = EspKeyValueStorage::new(EspNvs::new(partition, "upstream", true)?);
        let mut buffer = [0_u8; 512];
        let saved = storage
            .get_raw("config_v1", &mut buffer)?
            .and_then(|bytes| {
                serde_json::from_slice::<WifiCredentials>(bytes)
                    .ok()
                    .and_then(|value| value.validate().ok())
            });
        log::info!(
            "WAYPIN_LOCAL_NETWORK_READY: DHCP router=0 DNS=0; phone Internet remains on cellular"
        );
        Ok(Self {
            wifi,
            _ip_subscription: ip_subscription,
            storage,
            ap_storage,
            ap_configuration,
            saved,
            address: info.ip,
            subnet: info.subnet,
        })
    }

    pub fn display_label(&self) -> String {
        self.ap_configuration.ssid.to_string()
    }
    pub fn address(&self) -> Ipv4Addr {
        self.address
    }
    pub fn saved_credentials(&self) -> Option<WifiCredentials> {
        self.saved.clone()
    }

    fn upstream_connected(&self) -> bool {
        // is_connected() alone is also true for a running AP without a STA.
        matches!(
            self.wifi.get_configuration(),
            Ok(Configuration::Mixed(..) | Configuration::Client(..))
        ) && self.wifi.is_connected().unwrap_or(false)
    }

    fn station_configuration(&mut self, client: ClientConfiguration) -> Result<()> {
        self.wifi
            .set_configuration(&Configuration::Mixed(client, self.ap_configuration.clone()))?;
        local_dhcp(
            self.wifi.wifi().ap_netif(),
            sys::esp_netif_dhcp_option_mode_t_ESP_NETIF_OP_GET,
        )
    }

    pub fn scan(&mut self) -> Result<Vec<Value>> {
        if matches!(
            self.wifi.get_configuration()?,
            Configuration::AccessPoint(_)
        ) {
            self.station_configuration(ClientConfiguration::default())?;
        }
        let (found, _) = self.wifi.scan_n::<32>()?;
        let mut found = found
            .into_iter()
            .filter(|entry| !entry.ssid.is_empty() && entry.ssid != self.ap_configuration.ssid)
            .collect::<Vec<_>>();
        found.sort_by_key(|entry| std::cmp::Reverse(entry.signal_strength));
        let mut rows: Vec<Value> = Vec::new();
        for entry in found {
            let name = entry.ssid.to_string();
            if rows.iter().any(|row| row["ssid"] == name) {
                continue;
            }
            rows.push(
                json!({"ssid": name, "rssi": entry.signal_strength, "channel": entry.channel,
                "secured": entry.auth_method.is_some_and(|method| method != AuthMethod::None)}),
            );
        }
        Ok(rows)
    }

    pub fn connect_upstream(&mut self, value: WifiCredentials) -> Result<()> {
        let value = value.validate()?;
        if value.ssid == self.ap_configuration.ssid.as_str() {
            return Err("请选择其他 Wi-Fi，不能连接盒子自身热点".into());
        }
        let client = ClientConfiguration {
            ssid: value.ssid.as_str().try_into()?,
            password: value.password.as_str().try_into()?,
            auth_method: if value.password.is_empty() {
                AuthMethod::None
            } else {
                AuthMethod::WPA2Personal
            },
            ..Default::default()
        };
        if self.upstream_connected() {
            self.wifi.disconnect()?;
        }
        let result = (|| {
            self.station_configuration(client)?;
            self.wifi.connect()?;
            self.wifi.wait_netif_up()?;
            let info = self.wifi.wifi().sta_netif().get_ip_info()?;
            if subnets_overlap(self.subnet, info.subnet) {
                return Err("目标 Wi-Fi 与盒子热点网段冲突，请换一个网络".into());
            }
            esp!(unsafe {
                sys::esp_netif_set_default_netif(self.wifi.wifi().sta_netif().handle())
            })?;
            self.storage
                .set_raw("config_v1", &serde_json::to_vec(&value)?)?;
            self.saved = Some(value);
            Ok(())
        })();
        if result.is_err() {
            let _ = self.wifi.disconnect();
            let _ = self
                .wifi
                .set_configuration(&Configuration::AccessPoint(self.ap_configuration.clone()));
        }
        result
    }

    pub fn disconnect_upstream(&mut self) -> Result<()> {
        if self.upstream_connected() {
            self.wifi.disconnect()?;
        }
        self.wifi
            .set_configuration(&Configuration::AccessPoint(self.ap_configuration.clone()))?;
        local_dhcp(
            self.wifi.wifi().ap_netif(),
            sys::esp_netif_dhcp_option_mode_t_ESP_NETIF_OP_GET,
        )?;
        self.storage.remove("config_v1")?;
        self.saved = None;
        Ok(())
    }

    pub fn update_hotspot(&mut self, change: HotspotChange) -> Result<()> {
        let previous = HotspotConfig {
            ssid: self.ap_configuration.ssid.to_string(),
            password: self.ap_configuration.password.to_string(),
        };
        let next = change.resolve(&previous)?;
        if self
            .saved
            .as_ref()
            .is_some_and(|saved| saved.ssid == next.ssid)
        {
            return Err("热点名称不能与已保存的上游 Wi-Fi 相同".into());
        }
        if next == previous {
            return Ok(());
        }
        let mut ap = self.ap_configuration.clone();
        ap.ssid = next.ssid.as_str().try_into()?;
        ap.password = next.password.as_str().try_into()?;
        let mut old_radio = sys::wifi_config_t::default();
        esp!(unsafe {
            sys::esp_wifi_get_config(sys::wifi_interface_t_WIFI_IF_AP, &mut old_radio)
        })?;
        let mut radio = old_radio;
        // Updating the whole Mixed configuration would also rewrite the live
        // STA settings. Change only AP credentials and retain every other
        // radio setting, including the current upstream connection.
        unsafe {
            radio.ap.ssid.fill(0);
            radio.ap.ssid[..next.ssid.len()].copy_from_slice(next.ssid.as_bytes());
            radio.ap.ssid_len = next.ssid.len() as u8;
            radio.ap.password.fill(0);
            radio.ap.password[..next.password.len()].copy_from_slice(next.password.as_bytes());
        }
        self.ap_storage
            .set_raw("config_v1", &serde_json::to_vec(&next)?)?;
        let result = (|| -> Result<()> {
            esp!(unsafe {
                sys::esp_wifi_set_config(sys::wifi_interface_t_WIFI_IF_AP, &mut radio)
            })?;
            local_dhcp(
                self.wifi.wifi().ap_netif(),
                sys::esp_netif_dhcp_option_mode_t_ESP_NETIF_OP_GET,
            )
        })();
        if result.is_err() {
            let saved = self
                .ap_storage
                .set_raw("config_v1", &serde_json::to_vec(&previous)?)
                .is_ok();
            let restored = unsafe {
                sys::esp_wifi_set_config(sys::wifi_interface_t_WIFI_IF_AP, &mut old_radio)
            } == 0;
            return Err(if saved && restored {
                "热点修改失败，已恢复原设置，请重新连接原热点后重试"
            } else {
                "热点修改失败，请重启盒子，并检查新旧热点名称后重新连接"
            }
            .into());
        }
        self.ap_configuration = ap;
        // Re-authenticate existing clients with the new credentials. No MCU
        // reset, pairing deletion or upstream credential change is needed.
        if unsafe { sys::esp_wifi_deauth_sta(0) } != 0 {
            log::warn!("Hotspot updated; old clients may need to reconnect manually");
        }
        Ok(())
    }

    pub fn snapshot(&self) -> Value {
        let mut stations = sys::wifi_sta_list_t::default();
        let stations_ok = unsafe { sys::esp_wifi_ap_get_sta_list(&mut stations) } == 0;
        let clients = if stations_ok {
            stations.sta[..(stations.num.max(0) as usize).min(stations.sta.len())].iter().enumerate().map(|(i, station)| {
                json!({"index": i + 1, "rssi": station.rssi,
                    "id": format!("{:02X}:{:02X}:{:02X}", station.mac[3], station.mac[4], station.mac[5])})
            }).collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let mut channel = 0_u8;
        let mut secondary = 0;
        let _ = unsafe { sys::esp_wifi_get_channel(&mut channel, &mut secondary) };
        let connected = self.upstream_connected();
        let station = if connected {
            self.wifi.wifi().sta_netif().get_ip_info().ok()
        } else {
            None
        };
        json!({"ssid": self.ap_configuration.ssid.as_str(), "address": self.address.to_string(), "channel": channel,
            "clients": clients, "client_count": if stations_ok { Some(stations.num) } else { None },
            "local_only": local_dhcp(self.wifi.wifi().ap_netif(), sys::esp_netif_dhcp_option_mode_t_ESP_NETIF_OP_GET).is_ok(),
            "upstream": {"connected": connected, "saved_ssid": self.saved.as_ref().map(|value| &value.ssid),
                "address": station.as_ref().map(|info| info.ip.to_string()), "dns": station.as_ref().and_then(|info| info.dns.map(|ip| ip.to_string()))}})
    }
}

/// SET before starting DHCP; GET afterwards verifies the live server's flags.
/// ESP-IDF v5 uses a one-byte dhcps_offer_t for both options.
fn local_dhcp(netif: &EspNetif, mode: sys::esp_netif_dhcp_option_mode_t) -> Result<()> {
    for option in [
        sys::esp_netif_dhcp_option_id_t_ESP_NETIF_ROUTER_SOLICITATION_ADDRESS,
        sys::esp_netif_dhcp_option_id_t_ESP_NETIF_DOMAIN_NAME_SERVER,
    ] {
        let mut offered = 0_u8;
        esp!(unsafe {
            sys::esp_netif_dhcps_option(
                netif.handle(),
                mode,
                option,
                (&mut offered as *mut u8).cast(),
                size_of_val(&offered) as u32,
            )
        })?;
        if offered != 0 {
            return Err("Local Wi-Fi must not offer an Internet gateway or DNS server".into());
        }
    }
    Ok(())
}

fn subnets_overlap(left: ipv4::Subnet, right: ipv4::Subnet) -> bool {
    address_in_subnet(left.gateway, right) || address_in_subnet(right.gateway, left)
}

fn address_in_subnet(address: Ipv4Addr, subnet: ipv4::Subnet) -> bool {
    let mask = u32::from(Ipv4Addr::from(subnet.mask));
    u32::from(address) & mask == u32::from(subnet.gateway) & mask
}
