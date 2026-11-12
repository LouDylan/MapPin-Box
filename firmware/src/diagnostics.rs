use esp_idf_svc::sys;
use serde_json::{Value, json};
use std::ffi::CStr;

pub fn snapshot() -> Value {
    let internal = sys::MALLOC_CAP_INTERNAL | sys::MALLOC_CAP_8BIT;
    let external = sys::MALLOC_CAP_SPIRAM | sys::MALLOC_CAP_8BIT;
    let mut flash_bytes = 0;
    let flash_ok = unsafe { sys::esp_flash_get_size(std::ptr::null_mut(), &mut flash_bytes) } == 0;
    let reason = unsafe { sys::esp_reset_reason() };
    let reset_reason = match reason {
        sys::esp_reset_reason_t_ESP_RST_POWERON => "上电或硬件复位",
        sys::esp_reset_reason_t_ESP_RST_EXT => "外部复位",
        sys::esp_reset_reason_t_ESP_RST_SW => "软件重启",
        sys::esp_reset_reason_t_ESP_RST_PANIC => "程序异常",
        sys::esp_reset_reason_t_ESP_RST_INT_WDT
        | sys::esp_reset_reason_t_ESP_RST_TASK_WDT
        | sys::esp_reset_reason_t_ESP_RST_WDT => "看门狗复位",
        sys::esp_reset_reason_t_ESP_RST_BROWNOUT => "电压不足",
        sys::esp_reset_reason_t_ESP_RST_DEEPSLEEP => "深度睡眠唤醒",
        _ => "其他原因",
    };
    json!({
        "chip": "ESP32-S3", "cores": 2, "configured_cpu_mhz": 240,
        "idf_version": unsafe { CStr::from_ptr(sys::esp_get_idf_version()) }.to_string_lossy(),
        "flash_bytes": if flash_ok { Some(flash_bytes) } else { None },
        "psram_bytes": unsafe { sys::esp_psram_get_size() },
        "internal_free": unsafe { sys::heap_caps_get_free_size(internal) },
        "internal_min_free": unsafe { sys::heap_caps_get_minimum_free_size(internal) },
        "internal_largest_block": unsafe { sys::heap_caps_get_largest_free_block(internal) },
        "psram_free": unsafe { sys::heap_caps_get_free_size(external) },
        "reset_reason": reset_reason, "reset_code": reason
    })
}
