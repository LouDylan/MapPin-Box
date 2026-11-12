use crate::{iphone, persistent_storage::PersistentNvsPartition};
use enigma_embedded_core::Location;
use std::{error::Error, net::IpAddr};
use mappin_box_model::Place;

pub struct EspIdfBackend {
    storage: iphone::PairingStorage,
    controller: Option<iphone::IphoneController>,
    address: Option<IpAddr>,
    pairing_generation: u32,
}

impl EspIdfBackend {
    pub fn new(partition: PersistentNvsPartition) -> Result<Self, Box<dyn Error>> {
        Ok(Self {
            storage: iphone::pairing_storage(partition)?,
            controller: None,
            address: None,
            pairing_generation: iphone::pairing_generation(),
        })
    }

    fn refresh_pairing(&mut self) {
        let generation = iphone::pairing_generation();
        if generation != self.pairing_generation {
            self.controller.take();
            self.address = None;
            self.pairing_generation = generation;
        }
    }

    pub fn select_phone(&mut self, address: IpAddr) {
        self.refresh_pairing();
        if self.address != Some(address) {
            self.controller.take();
            self.address = Some(address);
            log::info!("iPhone location endpoint: {address}");
        }
    }

    fn connect_if_needed(&mut self) -> Result<(), Box<dyn Error>> {
        self.refresh_pairing();
        if self.controller.is_none() {
            let address = self
                .address
                .ok_or("请先在 iPhone 网页成功设置或恢复一次定位，再使用按键")?;
            self.controller = Some(iphone::IphoneController::connect(&self.storage, address)?);
        }
        Ok(())
    }

    pub fn set_location(&mut self, place: &Place) -> Result<(), Box<dyn Error>> {
        let result = (|| {
            self.connect_if_needed()?;
            let (latitude, longitude) = place.point()?;
            let location = Location::new(&place.name, latitude.to_string(), longitude.to_string());
            self.controller
                .as_mut()
                .expect("connected controller")
                .set(&location)
        })();
        if result.is_err() {
            self.controller.take();
        }
        result
    }

    pub fn restore_location(&mut self) -> Result<(), Box<dyn Error>> {
        let result = (|| {
            self.connect_if_needed()?;
            self.controller
                .as_mut()
                .expect("connected controller")
                .restore()
        })();
        if result.is_err() {
            self.controller.take();
        }
        result
    }
}
