# MapPin-Box 经典蓝牙 PAN 实验

这个实验让 ESP32-WROVER‑E 以 PANU 角色连接 iPhone 的个人热点。盒子取得 IP 后，检查热点网关的 TCP 49152（RemotePairing）和 62078（lockdownd），并查询两个 Bonjour 开发者服务。Safari 可访问串口打印的盒子网址，检查 iPhone 能否反向打开页面。探针只测试连通性。

实机已确认 iPhone 的 Safari 能通过蓝牙打开网页，盒子能访问手机的两个服务端口。正式固件保持 Wi-Fi 定位流程。蓝牙配对建立 IP 链路，开发者配对与定位服务还需要单独验证。

## 构建和测试

使用 [ESP-IDF 的固定提交](https://github.com/espressif/esp-idf/tree/c54fc57d386a4810be28add5e027968c04f910f1)。此提交有原生 PAN API，主固件使用的 ESP-IDF v5.5.3 没有该接口。激活该版本的 ESP32 工具环境后，从仓库根目录构建。

```sh
idf.py -C tools/bluetooth-pan-probe build
```

主固件已安装且分区表与探针完全一致时，只写探针的 `build/mappin_bluetooth_pan_probe.bin` 到工厂应用区 `0x20000`。保持 16 MB Flash 配置并保留 NVS。串口以 115200 打开，等待 `MAPPIN_BT_READY`。

1. 在 iPhone 开启蜂窝数据、个人热点的「允许其他人加入」和蓝牙，保持解锁并打开蓝牙设置。
2. 在串口输入 `scan`。扫描结束后，按手机名称选择地址，输入 `connect AA:BB:CC:DD:EE:FF`。
3. 如果手机弹出数字配对请求，对照串口的六位数字，在手机上确认，并在串口输入 `confirm 123456`，使用本次显示的号码。
4. 等待 `MAPPIN_BT_IP` 和端口检查结果。可在 Safari 打开串口显示的地址，也可输入 `check` 重试检查。
5. 测完输入 `disconnect`。需要恢复正式固件时，使用根目录的 WROVER‑E 烧录命令。

## 定位测试固件

同一 ESP-IDF 环境和 Xtensa Rust 工具链下，运行 `bash tools/bluetooth-pan-probe/location/build.sh`。定位测试固件的产物是 `location/build-output/app.bin`，分区表确认一致后可只写入 `0x20000`。它自动连接唯一已绑定的蓝牙设备；未连接时可用同一套串口命令重试。

已配对手机可通过物理串口临时载入远程配对资料。蓝牙 PAN 连通后，发送 `use-existing <字节数>` 和换行，收到 `MAPPIN_BT_EXISTING_INPUT` 后发送对应长度的 `RpPairingFile` 二进制数据。固件先认证手机并打开开发者定位通道；收到 `MAPPIN_BT_EXISTING_READY` 才开始改位测试。资料仅保存在本次运行的内存中。

2026-11-12 在 WROVER‑E 和 iOS 27.2 上实测：复用已有配对资料，盒子关闭 Wi-Fi，手机不接电脑。通过经典蓝牙 PAN 设置上海市中心（31.2304, 121.4737），并发送恢复指令；串口记录两条命令成功，苹果地图确认两次位置。测试中手动触发了 RST，重新认证后确认主动恢复成功。

首次开发者配对可从蓝牙网页发起。在 iOS 27 或更高版本开启开发者模式，打开蓝牙和个人热点，在网页点「开始配对」。到「设置 → 隐私与安全性 → 开发者模式」，在页面下方选择网页上显示的盒子，点「配对」。盒子收到请求后，回 Safari 查看六位码，再切回系统输入。配对窗口为五分钟。

握手在 PSRAM 任务中执行，成功后把配对记录交给使用内部内存的主任务写入 NVS。收到 `MAPPIN_BT_PAIR_DONE` 才算保存完成。WROVER‑E 与 iOS 27.2 已通过蓝牙首次开发者配对，并验证收到手机请求后才显示六位码、配对后设置和恢复定位。首次配对和定位服务准备仍是两件事；配对后需要打开手机的开发者定位服务验证。

局域网对照：没有经典蓝牙绑定且无配对资料的盒子会开启 `MapPin-Pair-Test`（`mappin-setup`），也可从串口输入 `setup-wifi`。用另一台设备加入该热点并打开 `http://192.168.4.1/`，让盒子连接普通的 2.4 GHz Wi-Fi，再点「开始配对」。iPhone 加入该普通 Wi-Fi 并从系统设置发起配对，在原网页查看验证码。网页控制入口只从临时热点和蓝牙开放。这个对照路径尚未完成真机配对验证。

点「定位到上海」并在苹果地图核对，再点「恢复真实定位」。测试固件在三分钟后也会尝试恢复，需保持连接。Apple 的 [无线开发者配对文档](https://developer.apple.com/documentation/xcode/managing-your-simulated-and-physical-devices-in-device-hub) 使用同一 Wi-Fi 网络，没有说明蓝牙 PAN 的支持情况。

电路接线见 [微雪原理图](https://files.waveshare.com/wiki/ESP32-DEV-KIT-XX/ESP32-DEV-KIT-XX.pdf)。协议参考 [Apple 支持的蓝牙配置文件](https://support.apple.com/en-us/102842) 和 [ESP-IDF PAN API](https://docs.espressif.com/projects/esp-idf/en/latest/esp32/api-reference/bluetooth/esp_pan.html)。
