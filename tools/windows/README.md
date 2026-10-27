# tools/windows — Windows 一键启动器

主程序是命令行的：需要手工开浏览器登录、粘回调 URL、补 DNS 路由，退出还要清残留。
这里的脚本把它包成「双击就能用」，并补上主程序目前缺的几件事。

| 文件 | 作用 |
|---|---|
| `xmu_vpn.exe`（由 `vpn_start.py` 打包，**不在仓库里**） | 双击入口：自动装 TAP 网卡 → 清理残留 → 启动 VPN → 补 DNS 路由 → 退出清理 |
| `vpn_start.py` | 上面那个 exe 的源码 |
| `vpn.cmd` | 备用入口（需要已安装 Python 3） |
| `tap_repair.py` / `tap_repair.cmd` | 单独安装 / 修复 TAP-Windows 虚拟网卡 |
| `netfix.py` / `网络修复.cmd` | 急救：清 NRPT 劫持、复位 TAP 网卡 DNS/IP、刷 DNS 缓存 |

## 补了主程序的哪些问题

1. **服务端下发的 DNS 没有隧道路由** → 连接后给 `210.34.0.18/14` 补 `/32` 路由
   （主程序侧修复见根目录 README 与 issue #1）。
2. **异常退出残留 NRPT / DNS** → 启动时先清一次，退出时再清一次；
   点 × 关窗口走快速清理（Windows 只给约 5 秒收尾），Ctrl+C 走完整清理。
3. **缺 TAP 网卡时只有一句 `cannot acquire TAP handle`** → 自动从 `tapdriver/` 安装。
4. **token 被服务端吊销后永远不重新登录** → 连接失败时把登录缓存改名并重试一次，
   并用**独立控制台**启动主程序（这样它才拿得到 stdin 粘回调 URL）。
   原因：主程序 `login(None)` 只看本地 JWT 的 `exp`，服务端返回 `2028`（登录过期）时
   本地 token 可能还没到期，于是它会一直认为「登录有效」。

## 日志

启动器把输出同时写进 `launcher.log`（同目录，超过 512KB 自动重置）。
出问题时把这个文件发出来即可，不需要抢截图。

## 打包 exe

```bash
pip install pyinstaller
python -m PyInstaller --onefile --name xmu_vpn --hidden-import tap_repair vpn_start.py
```

- `--hidden-import tap_repair` 必需：它在函数内动态 `import`，静态分析抓不到。
- 打包后 `__file__` 指向临时解压目录，脚本里用 `sys.frozen` 判断，改成以 exe 所在目录为准
  （`BASE`、提权重启、`tapdriver/` 查找三处都做了处理）。

## 注意

- 脚本内**没有硬编码路径**，整个文件夹拷到任何位置都能用。
- 登录缓存路径 `%LOCALAPPDATA%\MySecureLinkRs\data\session.json` 来自主程序
  `src/state.rs` 的 `ProjectDirs("", "", "MySecureLinkRs")`；主程序改应用名时需要同步这里。
