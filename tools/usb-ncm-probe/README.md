# MapPin-Box USB 实验

这里保留了两个通过 USB 网卡访问MapPin-Box的原型。iPhone 17 Pro 已用 USB-C 数据线打开原型网页。USB 定位和恢复尚未在真机上验证。正式固件通过 Wi-Fi 定位。

| 原型 | 构建 | 用途 |
| --- | --- | --- |
| USB 网络 | 激活 ESP-IDF v5.5.3 环境后运行 `bash tools/usb-ncm-probe/build.sh build` | 通过 USB 访问 `http://192.168.72.1`，查看枚举和网络状态 |
| USB 定位 | 配置好主项目的 Rust 工具链和 `espflash` 后运行 `bash tools/usb-ncm-probe/location/build.sh` | 用已有配对资料测试设到上海、恢复真实定位 |

先用主项目生成的固件确认板子的分区表一致。实验镜像只写工厂应用区 `0x20000`，保留默认 NVS、PHY 和 `userdata`。网络镜像位于 `build/mappin_usb_ncm_probe.bin`，定位镜像位于 `location/build-output/app.bin`。恢复主固件可使用根目录的烧录脚本。实验版不启动 Wi-Fi；实验结束后先恢复真实定位，再断线。

USB-C iPhone 连接板子的原生 USB 口。Lightning iPhone 需要带供电的 USB 主机转接器，此组合还没有实测。板子的 COM 口用于烧录；直接测试时拔掉 COM，避免同时供电。
