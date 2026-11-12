# Enigma

Waypin Box 使用 Enigma 的嵌入式串口配对协议、定位类型和持久化存储模块。源码来自 [Enigma 的固定提交](https://github.com/bruce-shi/enigma/tree/b50eb62b15f82ca42ebd427dcd7506e427a937ca)，按原路径保留如下文件。

- `apps/embedded/bridge-protocol/`
- `apps/embedded/core/`
- `apps/embedded/platforms/esp-idf/src/persistent_storage.rs`

根 `Cargo.toml` 仅保留这两个依赖 crate 的工作区配置，模块源码和 [上游许可证](LICENSE) 原样保留。固件的 `backend.rs`、`iphone.rs` 与 `wifi_access.rs` 也由 Enigma 的 ESP-IDF 实现演变而来，遵循同一许可证。
