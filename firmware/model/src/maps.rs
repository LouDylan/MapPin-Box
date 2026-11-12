use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Default, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MapProvider {
    #[default]
    Amap,
    Osm,
    Google,
    Tianditu,
}

impl MapProvider {
    pub fn global(self) -> bool {
        matches!(self, Self::Google | Self::Osm)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MapRegion {
    Domestic,
    Global,
}

impl MapRegion {
    fn matches(self, provider: MapProvider) -> bool {
        (self == Self::Global) == provider.global()
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct MapCredentials {
    pub key: String,
    pub security_code: String,
}

fn valid_key(value: &str) -> bool {
    (16..=128).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

impl MapCredentials {
    pub fn configured(&self) -> bool {
        valid_key(&self.key) && valid_key(&self.security_code)
    }

    pub fn validate(mut self) -> Result<Self, String> {
        self.key = self.key.trim().into();
        self.security_code = self.security_code.trim().into();
        if !self.configured() {
            return Err("请填写高德 Web端(JS API) Key 和安全密钥".into());
        }
        Ok(self)
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MapConfig {
    pub provider: MapProvider,
    pub domestic_provider: Option<MapProvider>,
    pub global_provider: Option<MapProvider>,
    pub amap: MapCredentials,
    pub google_key: String,
    pub tianditu_key: String,
}

#[derive(Deserialize)]
pub struct MapChange {
    pub provider: MapProvider,
    pub region: Option<MapRegion>,
    pub key: Option<String>,
    pub security_code: Option<String>,
}

impl MapConfig {
    pub fn seed_amap(&mut self, credentials: MapCredentials) -> Result<bool, String> {
        if self.amap.configured() {
            return Ok(false);
        }
        self.amap = credentials.validate()?;
        Ok(true)
    }

    pub fn migrate_regions(mut self) -> Self {
        self.global_provider = Some(MapProvider::Osm);
        if self.provider.global() {
            self.provider = MapProvider::Osm;
        }
        self
    }

    pub fn configured(&self) -> bool {
        self.has(self.provider)
    }

    pub fn has(&self, provider: MapProvider) -> bool {
        match provider {
            MapProvider::Amap => self.amap.configured(),
            MapProvider::Osm => true,
            MapProvider::Google => valid_key(&self.google_key),
            MapProvider::Tianditu => valid_key(&self.tianditu_key),
        }
    }

    pub fn change(&self, change: MapChange) -> Result<Self, String> {
        if change
            .region
            .is_some_and(|region| !region.matches(change.provider))
        {
            return Err("地图源与所选地区不匹配".into());
        }
        let mut next = self.clone();
        next.global_provider = next.global_provider.or(Some(MapProvider::Osm));
        if !self.provider.global() && next.domestic_provider.is_none() {
            next.domestic_provider = Some(self.provider);
        }
        if change
            .region
            .is_none_or(|region| region.matches(self.provider))
        {
            next.provider = change.provider;
        }
        let key = change.key.unwrap_or_default().trim().to_owned();
        let security_code = change.security_code.unwrap_or_default().trim().to_owned();
        match change.provider {
            MapProvider::Amap if !key.is_empty() || !security_code.is_empty() => {
                next.amap = MapCredentials { key, security_code }.validate()?;
            }
            MapProvider::Google if !key.is_empty() => next.google_key = key,
            MapProvider::Tianditu if !key.is_empty() => next.tianditu_key = key,
            _ => {}
        }
        if !next.has(change.provider) {
            return Err("请填写所选地图服务的凭证".into());
        }
        if change.provider.global() {
            next.global_provider = Some(change.provider);
        } else {
            next.domestic_provider = Some(change.provider);
        }
        Ok(next)
    }

    pub fn regional_providers(&self) -> serde_json::Value {
        let preferred = |global| {
            let saved = if global {
                self.global_provider
            } else {
                self.domestic_provider
            };
            let candidates = if global {
                [
                    saved,
                    Some(MapProvider::Osm),
                    Some(MapProvider::Google),
                    Some(self.provider),
                ]
            } else {
                [
                    saved,
                    Some(self.provider),
                    Some(MapProvider::Amap),
                    Some(MapProvider::Tianditu),
                ]
            };
            candidates
                .into_iter()
                .flatten()
                .find(|p| p.global() == global && self.has(*p))
        };
        serde_json::json!({"domestic": preferred(false), "global": preferred(true)})
    }

    pub fn available(&self) -> serde_json::Value {
        serde_json::json!({"amap": self.has(MapProvider::Amap), "osm": true,
            "google": self.has(MapProvider::Google), "tianditu": self.has(MapProvider::Tianditu)})
    }

    /// Only send the active provider's credentials to the browser.
    pub fn public(&self) -> serde_json::Value {
        let (key, security_code) = match self.provider {
            MapProvider::Amap => (self.amap.key.as_str(), self.amap.security_code.as_str()),
            MapProvider::Google => (self.google_key.as_str(), ""),
            MapProvider::Tianditu => (self.tianditu_key.as_str(), ""),
            MapProvider::Osm => ("", ""),
        };
        serde_json::json!({"provider": self.provider, "key": key, "security_code": security_code,
            "configured": self.configured(), "configured_providers": self.available(),
            "regional_providers": self.regional_providers()})
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usb_defaults_fill_missing_amap_without_changing_selected_maps_or_overwriting_keys() {
        let mut config = MapConfig {
            provider: MapProvider::Tianditu,
            domestic_provider: Some(MapProvider::Tianditu),
            global_provider: Some(MapProvider::Osm),
            tianditu_key: "t".repeat(32),
            ..Default::default()
        };
        let credentials = |value: &str| MapCredentials {
            key: value.repeat(32),
            security_code: "s".repeat(32),
        };
        assert!(config.seed_amap(credentials("a")).unwrap());
        assert!(!config.seed_amap(credentials("b")).unwrap());
        assert_eq!(config.amap.key, "a".repeat(32));
        assert_eq!(
            config.regional_providers(),
            serde_json::json!({"domestic":"tianditu","global":"osm"})
        );
        let mut fresh = MapConfig::default();
        assert!(fresh.seed_amap(MapCredentials::default()).is_err());
        assert!(!fresh.configured());
        assert!(fresh.seed_amap(credentials("a")).unwrap());
        assert!(fresh.configured());
    }

    #[test]
    fn switching_preserves_other_keys_and_reuses_saved_credentials() {
        let old = MapConfig {
            amap: MapCredentials {
                key: "k".repeat(32),
                security_code: "s".repeat(32),
            },
            ..Default::default()
        };
        let google = old
            .change(MapChange {
                provider: MapProvider::Google,
                region: None,
                key: Some("g".repeat(39)),
                security_code: None,
            })
            .unwrap();
        let osm = google
            .change(MapChange {
                provider: MapProvider::Osm,
                region: None,
                key: None,
                security_code: None,
            })
            .unwrap();
        let amap = osm
            .change(MapChange {
                provider: MapProvider::Amap,
                region: None,
                key: None,
                security_code: None,
            })
            .unwrap();
        assert_eq!(amap.amap.key, old.amap.key);
        assert_eq!(amap.google_key, google.google_key);
        assert!(amap.public().get("google_key").is_none());
        assert!(google.public().get("amap").is_none());
    }

    #[test]
    fn incomplete_changes_do_not_erase_working_configuration() {
        let old = MapConfig {
            amap: MapCredentials {
                key: "k".repeat(32),
                security_code: "s".repeat(32),
            },
            ..Default::default()
        };
        assert!(
            old.change(MapChange {
                provider: MapProvider::Amap,
                region: None,
                key: Some("new".repeat(11)),
                security_code: None
            })
            .is_err()
        );
        assert!(
            old.change(MapChange {
                provider: MapProvider::Tianditu,
                region: None,
                key: Some("short".into()),
                security_code: None
            })
            .is_err()
        );
        assert!(old.configured());
    }
    #[test]
    fn world_defaults_to_open_map_even_with_a_saved_google_key() {
        let mut value = MapConfig::default();
        assert_eq!(value.regional_providers()["global"], "osm");
        value.google_key = "g".repeat(39);
        assert_eq!(value.regional_providers()["global"], "osm");
        let value = value
            .change(MapChange {
                provider: MapProvider::Osm,
                region: None,
                key: None,
                security_code: None,
            })
            .unwrap();
        assert_eq!(value.regional_providers()["global"], "osm");
    }

    #[test]
    fn legacy_configuration_can_switch_regions_without_losing_the_domestic_choice() {
        let old = serde_json::json!({"provider":"tianditu", "tianditu_key":"t".repeat(32), "google_key":"g".repeat(39)});
        let value: MapConfig = serde_json::from_value(old).unwrap();
        assert_eq!(value.regional_providers()["domestic"], "tianditu");
        let world = value
            .change(MapChange {
                provider: MapProvider::Google,
                region: None,
                key: None,
                security_code: None,
            })
            .unwrap();
        // The first toggle must also remember the legacy active provider.
        assert_eq!(world.regional_providers()["domestic"], "tianditu");
    }

    #[test]
    fn regional_settings_do_not_switch_the_other_active_region() {
        let current = MapConfig {
            amap: MapCredentials {
                key: "k".repeat(32),
                security_code: "s".repeat(32),
            },
            ..Default::default()
        };
        let saved = current
            .change(MapChange {
                provider: MapProvider::Google,
                region: Some(MapRegion::Global),
                key: Some("g".repeat(39)),
                security_code: None,
            })
            .unwrap();
        assert!(saved.provider == MapProvider::Amap);
        assert_eq!(saved.regional_providers()["global"], "google");
        let world = saved
            .change(MapChange {
                provider: MapProvider::Google,
                region: None,
                key: None,
                security_code: None,
            })
            .unwrap();
        let domestic = world
            .change(MapChange {
                provider: MapProvider::Tianditu,
                region: Some(MapRegion::Domestic),
                key: Some("t".repeat(32)),
                security_code: None,
            })
            .unwrap();
        assert!(domestic.provider == MapProvider::Google);
        assert_eq!(domestic.regional_providers()["domestic"], "tianditu");
        assert_eq!(domestic.google_key, saved.google_key);
        assert_eq!(domestic.amap.key, current.amap.key);
    }

    #[test]
    fn wrong_region_is_rejected_without_changing_keys() {
        let current = MapConfig::default();
        assert!(
            current
                .change(MapChange {
                    provider: MapProvider::Google,
                    region: Some(MapRegion::Domestic),
                    key: Some("g".repeat(39)),
                    security_code: None,
                })
                .is_err()
        );
        assert!(current.google_key.is_empty());
    }

    #[test]
    fn legacy_migration_uses_open_map_and_preserves_all_credentials() {
        let current = MapConfig {
            provider: MapProvider::Google,
            global_provider: Some(MapProvider::Google),
            domestic_provider: Some(MapProvider::Tianditu),
            google_key: "g".repeat(39),
            tianditu_key: "t".repeat(32),
            ..Default::default()
        };
        let migrated = current.clone().migrate_regions();
        assert!(migrated.provider == MapProvider::Osm);
        assert_eq!(migrated.regional_providers()["global"], "osm");
        assert_eq!(migrated.regional_providers()["domestic"], "tianditu");
        assert_eq!(migrated.google_key, current.google_key);
        assert_eq!(migrated.tianditu_key, current.tianditu_key);
        let explicit = migrated
            .change(MapChange {
                provider: MapProvider::Google,
                region: Some(MapRegion::Global),
                key: None,
                security_code: None,
            })
            .unwrap();
        let reloaded: MapConfig =
            serde_json::from_slice(&serde_json::to_vec(&explicit).unwrap()).unwrap();
        assert_eq!(reloaded.regional_providers()["global"], "google");
    }
}
