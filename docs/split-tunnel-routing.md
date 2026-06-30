# SecureLink 分流代理：请求是如何走进隧道的

这份文档讲清楚「应用发出的请求」是怎么被引导进 VPN 隧道、最终到达内网资源的。SecureLink 的分流模型和标准 OpenVPN 不一样，理解这套机制后可以在别处借鉴。

---

## 1. 核心结论（一句话）

SecureLink **不下发任何标准路由**，而是把「哪些目标走隧道」以私有的 `app` 选项形式推给客户端。客户端必须自己把这些 `app` 目标**转换成指向 VPN 接口的主机路由**，操作系统才会把发往这些目标的报文交给 VPN 隧道。

---

## 2. 为什么不能直接用 OpenVPN 的路由

连接成功后，服务器的 `PUSH_REPLY` 里只有：

```
2  [route-gateway] [172.18.80.1]
158 [ifconfig] [172.18.82.116] [255.255.240.0]      ← 只配了 VPN 自己的 /20 子网
60 [app] [[addr:172.18.82.179/32][proto:any] [port:any]]
62 [app] [[addr:121.192.180.236/32][proto:any] [port:21]]
129 [app] [[addr:59.77.5.56/30][proto:any] [port:any]]
... （共约 90 条 app）
Add Routes:            ← 空！没有标准 route
Reroute Gateway: IPv4=0  ← 没有 redirect-gateway（不接管默认网关）
```

要点：

- **没有 `redirect-gateway`**：不接管全局默认路由，所以这是「分流（split-tunnel）」而不是「全局代理」。普通上网流量仍走本地网卡。
- **`Add Routes:` 为空**：服务器没有下发任何标准 `route` 指令。
- **真正的访问控制 / 分流清单藏在 `app` 选项里**：每条 `app` 描述一个允许走隧道的目标（`addr` = IP/前缀，可选 `proto`/`port`）。

OpenVPN3 核心不认识 `app`（我们在配置里写了 `ignore-unknown-option app ctrl`），于是它把这些当「未使用选项」忽略掉。结果：隧道建起来了，VPN 子网 `172.18.80.0/20` 能通，但像 `121.192.180.236`、`59.77.5.59` 这种**公网段的内网资源没有任何路由指向隧道** → 报文走默认网卡出去 → 在校外网络下根本到不了 → 表现为「显示已连接但访问不了资源」。

---

## 3. 完整数据流

```
   你的应用 (ftp / ssh / 浏览器)
        │  connect 121.192.180.236:21
        ▼
   操作系统路由表查询目的地
        │   ┌─ 命中我们安装的主机路由: 121.192.180.236/32 -> 接口11(DCO), nexthop 172.18.80.1
        ▼   │
   ovpn-dco-win 内核驱动 (接口 11)
        │  在内核里做 OpenVPN 数据通道封装/加密 (DCO = Data Channel Offload)
        ▼
   TCP-DCO 到 VPN 服务器 219.229.81.217:10000
        │  服务器解封装
        ▼
   厦大内网 → 121.192.180.236:21 (FTP) / 59.77.5.59:2222 (SSH)
        │  返回包原路加密回来
        ▼
   应用收到响应  ✅
```

关键点：**第 2 步那条主机路由是整套机制的开关**。没有它，第 1 步查路由表会命中默认网关，报文就走错了网卡。

---

## 4. 客户端实现（本项目做了什么）

分三步，全部在 `src/main.rs`：

### 4.1 从日志流里解析 `app` 目标 — `parse_app_route`

OpenVPN3 把「未使用选项」通过日志回调以多行消息发给我们（见 `src/ovpn3.rs` 的日志回调）。我们逐行扫描，凡是含 `[app]` 的行就抠出 `[addr:CIDR]`：

```
129 [app] [[addr:121.192.180.236/32][proto:any] [port:21]]
                  └──────────┬──────────┘
                      121.192.180.236/32
```

只取 IP/前缀，忽略 `proto`/`port`（路由是按目的 IP 匹配的，端口在这里不参与）。

### 4.2 从 CONNECTED 事件取接口索引与网关 — `parse_connected_info`

```
CONNECTED 34520...@219.229.81.217:10000 via /TCP-DCO
          on ovpn-dco-win/172.18.82.116/ gw=[172.18.80.1/] vpn_interface_index=11
                                              └─ 网关 ─┘            └ 接口索引 11 ┘
```

### 4.3 安装主机路由 — `install_app_routes`

对每个去重后的 `app` 目标执行（Windows）：

```
netsh interface ipv4 add route prefix=<CIDR> interface=<11> nexthop=172.18.80.1 store=active
```

- `interface=11`：把这条路由钉在 VPN 的 DCO 接口上。
- `nexthop=172.18.80.1`：VPN 网关（与 DCO 接口同段、on-link）。
- `store=active`：只写进活动路由表，不持久化（重启即失，符合「临时引流」语义）。
- 失败（多为「路由已存在」）当作良性，详细日志逐条打印 `+ 已加 / = 已存在 / ! 失败`。

> 「整个 `app` 列表都要进路由」——是的，全部去重后逐条安装，日志末尾给出 `done: N added, M already present, K failed`。例如 SSH 目标 `59.77.5.59` 本身没单列，但被 `59.77.5.56/30`（含 .56–.59）覆盖，所以也被正确引流。

---

## 5. 退出时的清理

应用优雅退出（Ctrl-C / 致命错误 / 服务器断开）时，`remove_app_routes` 会把装过的每条路由删掉：

```
netsh interface ipv4 delete route prefix=<CIDR> interface=<11> store=active
```

双保险说明：

- 即使进程被强杀（不走清理代码），这些路由钉在 DCO 接口上，接口断开后即失效；
- 而且**下次连接时 OpenVPN3 核心会先 `ActionDeleteAllRoutesOnInterface` 清空接口 11 上的所有路由**，所以不会无限累积。

---

## 6. 权限说明

- 安装/删除路由用的是 `netsh`，需要管理员权限——但这与 VPN 数据面本来就需要的权限是同一份，不额外增加要求。
- 我们**没有**改默认路由、**没有**接管全网流量，只对服务器授权的 `app` 目标加精确主机路由，影响面最小。

---

## 7. 验证方式（仅 debug 构建）

debug 构建会在 CONNECTED 后自动探测两个「只有走隧道才到得了」的内网主机，作为端到端证明（release 构建关闭探测）：

- FTP `121.192.180.236:21` → 期望 `230 User logged in`
- SSH `59.77.5.59:2222` → 期望 `SSH-2.0-OpenSSH_...` banner

在校外/非代理网络下这两个主机本不可达，能通即证明流量确实经隧道转发。

---

## 8. 如何在别处借鉴（通用套路）

这套「服务端下发 ACL → 客户端转成接口路由」的模式可以照搬到任何 split-tunnel 客户端：

1. **拿到允许清单**：可能来自专有 push 选项（本例的 `app`）、配置 API、或标准 `route` 指令。
2. **拿到隧道接口标识**：接口 index + 网关 IP（连接成功事件 / ifconfig 里都有）。
3. **逐条加主机路由**，钉在隧道接口上，nexthop 设隧道网关：
   - Windows：`netsh interface ipv4 add route prefix=… interface=… nexthop=… store=active`
   - Linux：`ip route add <cidr> dev <tun> [via <gw>]`
   - macOS：`route add -net <cidr> -interface <utun>`
4. **退出时按相同清单删除**，并依赖接口 teardown 兜底。
5. 想要「全局代理」而非分流，则改为下发/安装 `0.0.0.0/1` + `128.0.0.0/1`（等价 redirect-gateway，避免覆盖 `0.0.0.0/0` 与服务器路由打架）。

---

## 9. 相关代码位置

| 功能 | 文件 / 函数 |
| --- | --- |
| 解析 `app` 目标 | `src/main.rs` `parse_app_route` |
| 解析接口/网关 | `src/main.rs` `parse_connected_info` |
| 安装路由（详细日志） | `src/main.rs` `install_app_routes` |
| 退出清理路由 | `src/main.rs` `remove_app_routes` |
| 端到端探测（仅 debug） | `src/main.rs` `probe_ftp` / `probe_ssh` |
| OpenVPN3 日志回调（app 选项来源） | `src/ovpn3.rs` 日志 trampoline |
| 生成 profile / peer-info | `src/config.rs` `generate` |
