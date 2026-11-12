use esp_idf_svc::sys;
use sha2::{Digest, Sha256};
use std::error::Error;

// The on-disk marker also identifies backups made by pre-Waypin firmware.
const MAGIC: &[u8; 8] = b"I4BKUP01";
const USERDATA_SIZE: usize = 0x20000;

pub fn capture() -> Result<Vec<u8>, Box<dyn Error>> {
    let partition = unsafe {
        sys::esp_partition_find_first(
            sys::esp_partition_type_t_ESP_PARTITION_TYPE_DATA,
            sys::esp_partition_subtype_t_ESP_PARTITION_SUBTYPE_DATA_NVS,
            c"userdata".as_ptr(),
        )
        .as_ref()
    }
    .ok_or("找不到用户数据分区")?;
    if partition.size as usize != USERDATA_SIZE || partition.encrypted {
        return Err("当前用户数据分区不支持此备份格式".into());
    }

    let mut file = Vec::with_capacity(MAGIC.len() + USERDATA_SIZE + 32);
    file.extend_from_slice(MAGIC);
    file.resize(MAGIC.len() + USERDATA_SIZE, 0);
    let data = &mut file[MAGIC.len()..];
    let result =
        unsafe { sys::esp_partition_read(partition, 0, data.as_mut_ptr().cast(), data.len()) };
    if result != sys::ESP_OK {
        return Err("读取用户数据失败，请重试".into());
    }
    let checksum = Sha256::digest(&file);
    file.extend_from_slice(&checksum);
    Ok(file)
}
