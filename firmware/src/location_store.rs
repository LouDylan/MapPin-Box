use std::{error::Error, net::IpAddr};

use esp_idf_svc::nvs::{EspKeyValueStorage, EspNvs, NvsCustom};
use waypin_box_model::{Backup, Favorites, Place, record_recent};
use serde::{Serialize, de::DeserializeOwned};

use crate::persistent_storage::PersistentNvsPartition;

const BLOB_LIMIT: usize = 16384;
const FAVORITES_KEY: &str = "favorites_v2";
const HISTORY_KEY: &str = "history_v2";

pub struct LocationStore {
    storage: EspKeyValueStorage<NvsCustom>,
    pub favorites: Favorites,
    pub recent: Vec<Place>,
    pub phone_ip: Option<IpAddr>,
}

impl LocationStore {
    pub fn new(partition: PersistentNvsPartition) -> Result<Self, Box<dyn Error>> {
        let storage = EspKeyValueStorage::new(EspNvs::new(partition, "locations", true)?);
        let favorites: Favorites =
            read_versioned(&storage, FAVORITES_KEY, "favorites_v1")?.unwrap_or_default();
        favorites.validate()?;
        let recent = match read_versioned::<Vec<Place>>(&storage, HISTORY_KEY, "history_v1")? {
            Some(items) => items
                .into_iter()
                .map(Place::stored)
                .collect::<Result<_, _>>()?,
            None => migrate_recent(&storage)?,
        };
        Ok(Self {
            phone_ip: read_blob(&storage, "phone_v1")?,
            storage,
            favorites,
            recent,
        })
    }

    pub fn save(&mut self, id: Option<u32>, place: Place) -> Result<u32, Box<dyn Error>> {
        let mut next = self.favorites.clone();
        let id = next.put(id, place)?;
        write_blob(&self.storage, FAVORITES_KEY, &next)?;
        self.favorites = next;
        Ok(id)
    }

    pub fn delete(&mut self, id: u32) -> Result<(), Box<dyn Error>> {
        let mut next = self.favorites.clone();
        next.remove(id)?;
        write_blob(&self.storage, FAVORITES_KEY, &next)?;
        self.favorites = next;
        Ok(())
    }

    pub fn import(&mut self, backup: Backup) -> Result<(usize, usize), Box<dyn Error>> {
        let mut next = self.favorites.clone();
        let counts = next.merge(backup)?;
        write_blob(&self.storage, FAVORITES_KEY, &next)?;
        self.favorites = next;
        Ok(counts)
    }

    pub fn record(&mut self, place: Place) -> Result<(), Box<dyn Error>> {
        let mut next = self.recent.clone();
        record_recent(&mut next, place.stored()?);
        write_blob(&self.storage, HISTORY_KEY, &next)?;
        self.recent = next;
        Ok(())
    }

    pub fn clear_recent(&mut self) -> Result<(), Box<dyn Error>> {
        write_blob(&self.storage, HISTORY_KEY, &Vec::<Place>::new())?;
        self.recent.clear();
        Ok(())
    }

    pub fn forget_phone(&mut self) -> Result<(), Box<dyn Error>> {
        self.storage.remove("phone_v1")?;
        self.phone_ip = None;
        Ok(())
    }

    pub fn remember_phone(&mut self, address: IpAddr) -> Result<(), Box<dyn Error>> {
        if self.phone_ip != Some(address) {
            write_blob(&self.storage, "phone_v1", &address)?;
            self.phone_ip = Some(address);
        }
        Ok(())
    }
}

fn read_blob<T: DeserializeOwned>(
    storage: &EspKeyValueStorage<NvsCustom>,
    key: &str,
) -> Result<Option<T>, Box<dyn Error>> {
    let mut buffer = vec![0; BLOB_LIMIT];
    storage
        .get_raw(key, &mut buffer)?
        .map(serde_json::from_slice)
        .transpose()
        .map_err(Into::into)
}

fn read_versioned<T: DeserializeOwned>(
    storage: &EspKeyValueStorage<NvsCustom>,
    key: &str,
    legacy: &str,
) -> Result<Option<T>, Box<dyn Error>> {
    match read_blob(storage, key)? {
        Some(value) => Ok(Some(value)),
        None => read_blob(storage, legacy),
    }
}

fn write_blob<T: Serialize>(
    storage: &EspKeyValueStorage<NvsCustom>,
    key: &str,
    value: &T,
) -> Result<(), Box<dyn Error>> {
    let bytes = serde_json::to_vec(value)?;
    if bytes.len() > BLOB_LIMIT {
        return Err("地点数据超过存储上限".into());
    }
    storage.set_raw(key, &bytes)?;
    Ok(())
}

fn migrate_recent(storage: &EspKeyValueStorage<NvsCustom>) -> Result<Vec<Place>, Box<dyn Error>> {
    let mut history = Vec::new();
    for index in (0..6).rev() {
        let mut buffer = [0u8; 128];
        let Some(bytes) = storage.get_raw(&format!("recent{index}"), &mut buffer)? else {
            continue;
        };
        let Ok(text) = std::str::from_utf8(bytes) else {
            continue;
        };
        let fields: Vec<_> = text.split('\u{1f}').collect();
        if fields.len() != 3 {
            continue;
        }
        let (Ok(latitude), Ok(longitude)) = (fields[1].parse(), fields[2].parse()) else {
            continue;
        };
        if let Ok(place) = (Place {
            name: fields[0].into(),
            latitude: Some(latitude),
            longitude: Some(longitude),
            ..Default::default()
        })
        .validate()
        {
            record_recent(&mut history, place);
        }
    }
    write_blob(storage, HISTORY_KEY, &history)?;
    Ok(history)
}
