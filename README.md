# MCHOSE HUB for Linux

MCHOSE K99 V2 的 Linux 原生驱动设置程序。使用 Rust + GTK4 + libadwaita 编写，不依赖 Electron、Wine 或浏览器 WebHID。

> **非官方项目**：本项目与迈从科技（MCHOSE）无隶属关系。协议与型号数据是通过公开的 HUB 网页客户端分析得到的互操作信息，仓库中不包含官方二进制、前端代码或图片资源。

**状态：v0.1.0，K99 V2 的 USB 有线与 2.4G 接收器均已在真机验证（2026-10-25）。**

![设备页（模拟设备）](docs/screenshots/device.png)

## 功能

- **设备信息**：型号、当前连接方式、USB 有线 / 2.4G 接收器的发现状态、电量、回报率、当前灯效、休眠、Win 锁、Mac 模式。
- **双连接发现**：两种接口同时插入时分别显示“已检测到 / 未检测到”；优先使用 USB 有线，有线无法打开时回退到 2.4G 接收器。无线连接期间插入 USB 线会自动切换到有线。
- **按键**：原生绘制 K99 V2 99 键布局（按功能布局重绘，导航键/方向键的物理位置以实际键盘为准），支持默认层 / Fn 层 / Fn2 层；可改键为键盘键、修饰键、鼠标键、媒体键、快捷功能和禁用。
- **灯效**：常亮、呼吸、梦幻彩虹、一触即发、雨中漫步、涟漪扩散、繁星流彩、随波逐流、潮汐共振、光影、正弦波、风车、瀑布、繁花、自定义、关闭；可调颜色、亮度、速度、多彩。
- **性能**：250/500/1000 Hz 回报率、低延迟模式、休眠时间、Win 键锁、Mac 布局（软件交换 Alt/Win）。
- **恢复默认**：按键分层恢复、性能恢复、灯效颜色恢复。
- **模拟设备**：无键盘时可完整演示和测试 UI：`--simulate`。
- **诊断工具**：`--probe` 只读导出，`--probe-write` 原样写回并校验。

| 按键 | 灯效 |
|---|---|
| ![按键页](docs/screenshots/keys.png) | ![灯效页](docs/screenshots/lighting.png) |

| 性能 | 设置 |
|---|---|
| ![性能页](docs/screenshots/performance.png) | ![设置页](docs/screenshots/settings.png) |

*截图为模拟设备与暗色主题。*

## 支持范围与限制

支持：

- MCHOSE K99 V2，通过 **USB 有线** 或 **Compx 2.4G Wireless Receiver** 连接；两者同时插入时优先使用有线，设备页会同时显示两种接口的发现状态。
- 设备在 Linux 中会伪装成 `05ac:024f`（Apple Aluminium Keyboard）。本程序通过产品字符串与厂商 HID 报告识别：无线接收器使用 Report ID 19 的 20 字节输出报告，有线接口使用 Report ID 6 的 519 字节 Feature Report，不会误接管真正的 Apple 键盘。

暂不支持：

- 其他 MCHOSE 型号、磁轴键盘（需要另一套协议与型号数据）。
- 固件刷写 / OTA。
- 宏编辑、云端配置、音乐律动、Windows 音频驱动、THX 等功能。
- Mac 布局只交换默认层的 Alt/Win（软件实现），不会改变 F1-F12；完整固件 Mac 模式请按键盘的 **Fn + S / Fn + A** 切换。
- 自定义灯效（模式 21）的逐键颜色编辑；该模式会保留键盘中原有的自定义灯效。

无线模式下键盘休眠后厂商通道会停止响应。**按键盘任意键唤醒，再点击右上角刷新按钮。**

## 构建与运行

依赖：

- Rust stable，建议使用 [mise](https://mise.jdx.dev/)：`mise install`
- GTK 4.18+ 与 libadwaita 1.6+ 的开发包（pkg-config 能找到 `gtk4` 与 `libadwaita-1`）
- Meson / Ninja（可选，用于安装与 Flatpak）

Ubuntu / Debian：

```bash
sudo apt install libgtk-4-dev libadwaita-1-dev meson ninja-build
```

Fedora：

```bash
sudo dnf install gtk4-devel libadwaita-devel meson ninja-build
```

直接用 Cargo 构建：

```bash
mise install
cargo build --release
./target/release/mchose_hub_linux            # 真机
./target/release/mchose_hub_linux --simulate # 模拟设备
./target/release/mchose_hub_linux --help     # 帮助
```

如果系统没有 root 权限安装 libadwaita 开发包，可直接用 GNOME Builder / Flatpak 构建（清单已配置好），或把用户态解包的 dev 包加到 `PKG_CONFIG_PATH` 后再用 Cargo 构建。

用 Meson 安装：

```bash
meson setup build --prefix="$HOME/.local"
meson compile -C build
meson install -C build
```

Flatpak / GNOME Builder：

```bash
flatpak-builder --user --install --force-clean builddir top.ddmt.mchosehub.json
```

清单已加入 `--device=all`，用于访问 `/dev/hidraw*`（范围较广，发布前可按需收紧）。需要 `org.gnome.Sdk` 50（或可用版本）与 `org.freedesktop.Sdk.Extension.rust-stable`。

## HID 权限（udev）

部分发行版默认只允许 root 读取 hidraw。安装规则：

```bash
sudo cp udev/70-mchose-hub.rules /etc/udev/rules.d/
sudo udevadm control --reload-rules
sudo udevadm trigger
```

然后重新插拔键盘。规则内容：

```udev
SUBSYSTEM=="hidraw", ATTRS{idVendor}=="05ac", ATTRS{idProduct}=="024f", TAG+="uaccess"
```

应用内“设置”页也可以复制该规则。当前开发机上 systemd-logind 已通过 ACL 授予当前用户访问权限，因此无需额外配置。

## 使用

1. 用 USB 线连接 K99 V2，或插入 2.4G 接收器；无线时按一下键盘任意键唤醒。
2. 启动程序，等待设备页读取完成。设备页会分别显示 USB 有线与 2.4G 接收器是否被检测到；两种连接同时存在时优先使用有线，无线期间插入 USB 线也会自动切换到有线。
3. 在“按键 / 灯效 / 性能”页修改；每次修改立即写入键盘，并在写入后回读校验。
4. 若提示超时，按任意键唤醒键盘后点击刷新。

修改是逐项立即生效的，没有“保存”按钮。顶部错误提示条与底部 Toast 会显示写入结果或错误。

## 诊断命令

```bash
# 只读：打印传输方式、电量、性能、灯效颜色、三层键表
./target/release/mchose_hub_linux --probe

# 读取后把性能、三层键表、灯效原样写回并校验
# 不会改变当前设置，用于验证 HID 链路
./target/release/mchose_hub_linux --probe-write
```

`--probe` 与 `--probe-write` 不启动 GUI，适合排查权限或休眠问题。`--help` 与 `--version` 输出用法和版本。

## 测试

```bash
cargo test
```

覆盖：

- 报文编码/解码、校验和、分包与重组；
- 有线 Feature Report 的 519 字节请求布局、响应头校验与 HID 描述符识别；
- 双接口发现、优先有线以及有线无法打开时的无线回退；
- 设备信息/性能字段偏移、灯效颜色偏移、三层键表编码；
- 图层选择字节与 YY 编码回归；
- 模拟设备服务连接事件。

真机验证（K99 V2）：

- USB 有线：识别 519 字节 Feature Report，读取电量、性能、灯效、默认 / Fn / Fn2 三层键表；性能、三层键表、灯效原样写回并逐字节回读一致；
- 2.4G 无线：读取电量、性能、灯效、三层键表；性能、三层键表、灯效原样写回并逐字节回读一致；
- UI 中 Win 键锁开→关，设备页回读状态一致；
- 设备页同时显示两类接口的发现状态：仅接收器时显示“2.4G 接收器 已检测到 · hidraw5”和“USB 有线 未检测到”（Xvfb + 真机 + OCR 验证）；
- 有线优先、无线回退与无线期间插线自动切换由选择逻辑单元测试覆盖；双接口同时插入的真机场景尚未复测。

UI 在 Xvfb + 模拟设备下逐页截图并用 OCR 验证了设备页、键盘绘制、动作选择器、灯效页、性能页与写入事件。

## 架构

```
src/
  model.rs            型号数据（K99 V2 布局/键表/动作/默认值）
  protocol.rs         报文、校验、分包、性能/键表/灯效结构
  device/
    hidraw.rs         hidraw 枚举、无线/有线识别、Feature Report ioctl
    link.rs           无线 RealLink、有线 WiredLink、ACK/回读校验
    mock.rs           模拟设备
    service.rs        后台设备线程、事件/请求
  ui/
    mod.rs            Adw 窗口、五个页面、事件驱动
    keyboard.rs       原生可缩放键盘控件
data/models/k99_v2.json
docs/protocol.md      协议说明
```

协议要点（详见 [docs/protocol.md](docs/protocol.md)）：

- 无线：HID Report ID `0x13`，每次 20 字节输出报告；载荷 19 字节 = `[命令, 总包数, 包序号, YY, 14 字节数据, 校验和]`；校验和 = Report ID + 前 18 字节之和（mod 256）；
- 有线：HID Report ID `0x06`，519 字节 Feature Report；`[命令, 5 字节 space0, 数据, 0 填充]`，使用 `HIDIOCSFEATURE/HIDIOCGFEATURE`（缓冲区 520 字节，含 Report ID）；
- 键盘键表 113 槽 × 4 字节，写入时追加 58 字节 0 与 `0x5A 0xA5`；
- 无线图层通过 YY 高半字节区分（0=默认，1=Fn，2=Fn2），低半字节为本包有效长度；有线图层放在 space0 首字节。

## 数据来源与许可

- 协议与型号数据来自对公开 HUB 网页客户端的静态分析，仅用于互操作与 Linux 兼容；仓库不包含官方代码、二进制或图片。
- 键位、HID 码、默认键值等为设备事实数据；UI、Rust 实现与协议文档为本项目原创。
- 界面符号图标来自 GNOME Adwaita 图标主题（CC-BY-SA-3.0/4.0 或 LGPL-3.0，署名 GNOME Project），已内置到应用资源，命名参考 GNOME Icon Library 的 dev-kit；详见 `data/icons/mchose/README.md`。
- 许可证：GNU GPL-3.0-or-later（见 `COPYING`）。

## 路线图

- 宏读取/编辑与按键绑定；
- 22 款磁轴键盘（Ace / Jet / K / Mix / G / ZERO 系列）；
- 更多遗留型号；
- 英文界面与翻译。

欢迎提交 issue / PR，尤其是其他型号的 `--probe` 输出与 HID 描述符。

## 发布前检查

- 在 `data/top.ddmt.mchosehub.metainfo.xml.in` 中补充项目主页 `url`，并重新启用 `data/meson.build` 中的 AppStream 校验。
- 根据目标发行版收紧 Flatpak 的 `--device=all` 权限。
- 补充英文翻译或说明。
