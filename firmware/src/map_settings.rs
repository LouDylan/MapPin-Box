use crate::persistent_storage::PersistentNvsPartition;
use esp_idf_svc::nvs::{EspKeyValueStorage, EspNvs, NvsCustom};
use waypin_box_model::{MapChange, MapConfig, MapCredentials};
use std::error::Error;

pub struct MapSettings {
    storage: EspKeyValueStorage<NvsCustom>,
    value: MapConfig,
}

impl MapSettings {
    pub fn new(partition: PersistentNvsPartition) -> Result<Self, Box<dyn Error>> {
        let storage = EspKeyValueStorage::new(EspNvs::new(partition, "maps", true)?);
        let value = Self::load(&storage)?;
        Ok(Self { storage, value })
    }

    fn load(storage: &EspKeyValueStorage<NvsCustom>) -> Result<MapConfig, Box<dyn Error>> {
        let mut buffer = [0_u8; 2048];
        if let Some(bytes) = storage.get_raw("config_v3", &mut buffer)? {
            return Ok(serde_json::from_slice(bytes)?);
        }
        if let Some(bytes) = storage.get_raw("config_v2", &mut buffer)? {
            return Ok(serde_json::from_slice::<MapConfig>(bytes)?.migrate_regions());
        }
        let amap = storage
            .get_raw("config_v1", &mut buffer)?
            .and_then(|bytes| serde_json::from_slice::<MapCredentials>(bytes).ok())
            .and_then(|value| value.validate().ok())
            .unwrap_or_default();
        Ok(MapConfig {
            amap,
            ..Default::default()
        })
    }

    pub fn get(&self) -> MapConfig {
        self.value.clone()
    }

    pub fn save(&mut self, change: MapChange) -> Result<(), Box<dyn Error>> {
        let value = self.value.change(change)?;
        self.storage
            .set_raw("config_v3", &serde_json::to_vec(&value)?)?;
        self.value = value;
        Ok(())
    }
}
