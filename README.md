> **这是 [XMU-MoYu-Club/xmu_secure_link](https://github.com/XMU-MoYu-Club/xmu_secure_link) 的个人 fork。**
>
> 在原版基础上修了一个影响非校园网使用的问题：服务端通过 `dhcp-option DNS` 下发的 DNS 服务器
> 没有被加入 split-tunnel 路由，导致校外连接后所有域名解析都要等约 12 秒超时，网页全部打不开。
> 本 fork 把推送的 DNS 地址一并安装为隧道的 `/32` 主机路由（`src/main.rs`，含 5 个单元测试）。
>
> 上游 issue：[#1](https://github.com/XMU-MoYu-Club/xmu_secure_link/issues/1) ·
> 对应 PR：[#3](https://github.com/XMU-MoYu-Club/xmu_secure_link/pull/3)
>
> 如果上游已经合并了该修复，请优先使用上游版本。

---

# xmu_secure_link

厦门大学 SecureLink（svpnlink.xmu.edu.cn）的第三方命令行 VPN 客户端，用 **Rust** 编写。把 **OpenVPN3 核心从源码编译**进来作为进程内库（FFI），交互方式对齐官方客户端：浏览器登录 → 粘贴回调 URL → 自动连接并按服务器下发的清单分流。

单一可执行文件，数据面是编译进来的 OpenVPN3，无需外部 `openvpn.exe`。

---

## ✨ 特性

- **进程内 OpenVPN3 数据面**：`vendor/openvpn3`（git submodule）由 `build.rs` + CMake + vcpkg 编译成共享库，Rust 经 C ABI 驱动，单进程完成连接。
- **SSO 登录**：走统一身份认证，浏览器打开登录页 → 粘贴回调 URL → 缓存 token，下次自动刷新。
- **分流路由**：解析服务器下发的分流清单，为每个目标安装走 VPN 接口的路由，退出时清理。原理见 [`docs/split-tunnel-routing.md`](docs/split-tunnel-routing.md)。
- **Windows 自动提权**：可执行文件内嵌 UAC 清单，双击即申请管理员权限（创建 tun 适配器与安装路由所需）。

---

## 🗺️ 工作流程

```
登录 (SSO)            ──►  浏览器打开登录页，粘贴回调 URL，拿到并缓存 token
   │
取配置 + 生成 profile ──►  从服务器配置生成 OpenVPN3 profile / 凭据
   │
连接 (OpenVPN3 core)  ──►  TLS 握手 → 认证 → CONNECTED，拿到 VPN IP / 接口 / 网关
   │
安装分流路由          ──►  把服务器下发的分流目标加成走 VPN 接口的主机路由
   │
   ▼
匹配的请求经隧道转发到内网；Ctrl-C 断开并清理路由
```

---

## 🚀 构建

依赖：Rust（MSVC toolchain）、Visual Studio（MSVC）、CMake + **Ninja**、vcpkg。

```bash
# 一次性安装 OpenVPN3 依赖
vcpkg install asio fmt jsoncpp lz4 openssl tap-windows6 xxhash pkgconf --triplet x64-windows

git clone --recursive https://github.com/XMU-MoYu-Club/xmu_secure_link.git
cd xmu_secure_link

export VCPKG_ROOT=/path/to/vcpkg     # Windows: 设为 vcpkg 安装目录
cargo build --release
```

`build.rs` 会自动编译并链接 OpenVPN3，并把运行所需的共享库拷到可执行文件旁边。产物在 `target/release/`（`xmu_secure_link(.exe)` + `ovpnffi` 及其依赖库）。

---

## ▶️ 使用

```bash
./target/release/xmu_secure_link
```

Windows 下双击会弹出 UAC 申请管理员权限。运行后按提示完成浏览器登录并粘贴回调 URL，即自动连接；`Ctrl-C` 断开。

> debug 构建会在连接后自动探测内网资源做端到端自检；release 构建关闭。

---

## 📁 项目结构

```
src/         控制面：交互、SSO API、配置生成、OpenVPN3 的 FFI 封装、分流路由
native/      C++ shim：子类化 OpenVPNClient 暴露 C ABI + CMake
vendor/openvpn3   OpenVPN3 core (git submodule)
build.rs     用 cmake + vcpkg 编译 native，链接并拷贝运行库，嵌入 UAC 清单
docs/        分流路由原理与借鉴指南
```

---

## 🙏 致谢

- **[XMU-MoYu-Club/MySecureLink](https://github.com/XMU-MoYu-Club/MySecureLink)** —— 研究并攻克了 SecureLink 的加密/解密握手过程，本项目的认证协议实现基于其研究成果。
- 本仓库在此基础上实现了其余周边内容：Rust 重写、从源码编译集成 OpenVPN3 核心库、交互方式、分流路由、跨平台与打包发布。
- [OpenVPN/openvpn3](https://github.com/OpenVPN/openvpn3) —— 数据面核心库。

---

## 📄 许可证

[GNU General Public License v3.0](LICENSE)。`vendor/openvpn3` 遵循其自身许可证。
