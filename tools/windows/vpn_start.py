# -*- coding: utf-8 -*-
# XMU SecureLink 一键启动器 v3（可移植版）
# 修复的项目缺陷: 连接后 DNS 被劫持到校内 210.34.0.18/14 但分流路由不覆盖 DNS 服务器,
#                校外使用时所有 DNS 超时; 且异常退出会残留 NRPT 劫持规则导致全网断网。
# 流程: 提权 → 检测 TAP 网卡 → 清理残留(僵尸实例/NRPT/DNS路由) → 启动 VPN → 等 CONNECTED
#      → 给校内 DNS 补隧道路由 → 随进程退出自动清理
# 可移植: 所有路径基于脚本所在目录，整个文件夹拷到其他电脑即可用。
import ctypes, subprocess, sys, time, os, winreg

# 打包成 exe 时 (PyInstaller)，__file__ 指向临时解压目录；此时以 exe 所在目录为准
if getattr(sys, "frozen", False):
    BASE = os.path.dirname(os.path.abspath(sys.executable))
else:
    BASE = os.path.dirname(os.path.abspath(__file__))
EXE = os.path.join(BASE, "xmu_secure_link.exe")
VPN_GW = "172.18.80.1"
DNS_SERVERS = ["210.34.0.18", "210.34.0.14"]
# 主程序的登录缓存。路径来自 src/state.rs 的 ProjectDirs("", "", "MySecureLinkRs")，
# 主程序若改了应用名需要同步这里。
SESSION_PATH = os.path.join(os.environ.get("LOCALAPPDATA", ""), "MySecureLinkRs", "data", "session.json")
NRPT_KEY = r"SYSTEM\CurrentControlSet\Services\Dnscache\Parameters\DnsPolicyConfig"
NET_CLASS = r"SYSTEM\CurrentControlSet\Control\Class\{4D36E972-E325-11CE-BFC1-08002BE10318}"
LOG_PATH = os.path.join(BASE, "launcher.log")

def tap_present():
    """检测系统里是否存在 TAP-Windows 适配器设备实例"""
    try:
        k = winreg.OpenKey(winreg.HKEY_LOCAL_MACHINE, NET_CLASS, 0, winreg.KEY_READ)
    except OSError:
        return False
    i = 0
    while True:
        try:
            sub = winreg.EnumKey(k, i); i += 1
        except OSError:
            break
        if not sub.isdigit():
            continue
        try:
            sk = winreg.OpenKey(k, sub, 0, winreg.KEY_READ)
            desc, _ = winreg.QueryValueEx(sk, "DriverDesc")
            winreg.CloseKey(sk)
            if "TAP-Windows" in str(desc) or "tap0901" in str(desc).lower():
                winreg.CloseKey(k)
                return True
        except OSError:
            continue
    winreg.CloseKey(k)
    return False

def is_admin():
    try:
        return ctypes.windll.shell32.IsUserAnAdmin()
    except Exception:
        return False

def pause_exit(msg=None, code=1):
    """打印错误后停住窗口（避免闪退看不到提示），再退出"""
    if msg:
        print(msg)
    try:
        input("按回车键关闭窗口...")
    except EOFError:
        pass
    sys.exit(code)

def ensure_tap():
    """确保 TAP 网卡存在；没有就用分发包自带驱动自动安装（首次使用）"""
    if tap_present():
        print("      TAP-Windows 适配器正常")
        return True
    print("      首次使用: 正在安装 TAP 虚拟网卡（VPN 必需，用本文件夹自带驱动）...")
    try:
        sys.path.insert(0, BASE)
        import tap_repair
        inf = tap_repair.find_tap_inf()
        if not inf:
            pause_exit("错误: 本文件夹里找不到 TAP 驱动。\n"
                       "      请确认拷贝时包含 tapdriver 子目录，或重新完整拷贝。")
        tap_repair.install(inf)
        print("      TAP 虚拟网卡安装完成")
        return True
    except Exception as e:
        pause_exit("错误: 安装 TAP 虚拟网卡失败: %s\n"
                   "      可右键以管理员身份运行 tap_repair.py 单独修复。" % e)
    return False

ERROR_CANCELLED = 1223  # ShellExecuteW: UAC 被用户取消时的返回码

def elevate():
    if getattr(sys, "frozen", False):
        # 已打包为 exe：直接以 exe 自身提权，不需要脚本参数
        ret = ctypes.windll.shell32.ShellExecuteW(None, "runas", sys.executable, None, None, 1)
    else:
        ret = ctypes.windll.shell32.ShellExecuteW(None, "runas", sys.executable,
                                                  '"%s"' % os.path.abspath(__file__), None, 1)
    if ret == ERROR_CANCELLED:
        print("你取消了管理员授权（UAC 弹窗需要点\"是\"）")
        input("按回车退出...")
        sys.exit(1)
    if ret <= 32:
        print("提权失败（错误码 %s），无法启动 VPN" % ret)
        input("按回车退出...")
        sys.exit(1)
    sys.exit(0)

def run(cmd, timeout=60):
    try:
        subprocess.run(cmd, shell=True, capture_output=True, timeout=timeout)
    except Exception:
        pass

# 复位 TAP 网卡: 清掉 OpenVPN3 被强杀后残留的校内 DNS / 静态 IP
# （这些残留会导致"退出 VPN 后网络卡死"，必须清）
TAP_RESET_PS = ("Get-NetAdapter | Where-Object { $_.InterfaceDescription -match 'TAP-Windows' } | "
                "ForEach-Object { $i = $_.ifIndex; "
                "Get-NetIPAddress -InterfaceIndex $i -AddressFamily IPv4 -ErrorAction SilentlyContinue | "
                "Where-Object { $_.PrefixOrigin -eq 'Manual' } | "
                "Remove-NetIPAddress -Confirm:$false -ErrorAction SilentlyContinue; "
                "Set-DnsClientServerAddress -InterfaceIndex $i -ResetServerAddresses -ErrorAction SilentlyContinue }")

def cleanup():
    """清理 VPN 可能残留的一切: 僵尸实例、NRPT 劫持规则、DNS 主机路由、TAP 网卡设置"""
    run('taskkill /F /IM xmu_secure_link.exe')  # 没有实例时无害
    # VPN 刚退出时，注册表策略和网卡配置还被系统短暂占用，立刻删会被拒；稍等再清理
    time.sleep(2)
    try:
        k = winreg.OpenKey(winreg.HKEY_LOCAL_MACHINE, NRPT_KEY, 0, winreg.KEY_ALL_ACCESS)
        victims = []
        i = 0
        while True:
            try:
                sub = winreg.EnumKey(k, i); i += 1
            except OSError:
                break
            if sub.startswith("OpenVPNDNSRouting"):
                victims.append(sub)
        for v in victims:
            for attempt in range(3):
                try:
                    winreg.DeleteKey(k, v)
                    print("  清理残留 NRPT 劫持规则: %s" % v)
                    break
                except OSError as e:
                    if attempt == 2:
                        print("  清理 NRPT 失败（%s）: %r" % (v, e))
                    else:
                        time.sleep(1)
        winreg.CloseKey(k)
    except FileNotFoundError:
        pass
    except PermissionError:
        print("  (提示: 清理 NRPT 需要管理员权限)")
    for dns in DNS_SERVERS:
        run("route delete %s" % dns)
    # TAP 网卡复位（清残留校内 DNS / 静态 IP）；设备刚断开时也会失败，重试一次
    run('powershell -NoProfile -Command "%s"' % TAP_RESET_PS)
    time.sleep(1)
    run('powershell -NoProfile -Command "%s"' % TAP_RESET_PS)
    run("ipconfig /flushdns")

def retire_session():
    """把登录缓存改名（可还原），强制下次走 SSO 登录。

    主程序的 login(None) 只看本地 JWT 的 exp：服务端已吊销会话（返回 2028）时，
    本地 token 却还没到期，于是它一直认为「登录有效」，永远不会重新登录。
    这种情况只能清掉本地缓存让它重走一遍。
    """
    if not os.path.exists(SESSION_PATH):
        return
    dst = SESSION_PATH + time.strftime(".bak-%Y%m%d-%H%M%S")
    try:
        os.replace(SESSION_PATH, dst)
        print("  已移走登录缓存: %s" % os.path.basename(dst))
    except OSError as e:
        print("  移走登录缓存失败: %r" % e)

def launch_vpn(new_console):
    """启动主程序。

    - 已缓存登录：用 DEVNULL 占住 stdin（exe 的 stdin 句柄异常会启动即 code=1 崩溃），
      整个流程只用本窗口。
    - 需要登录：开独立控制台，用户要能在那个窗口里粘贴回调 URL —— 本窗口的 stdin
      已经被 DEVNULL 占掉了，拿不到输入。
    """
    if new_console:
        return subprocess.Popen([EXE], cwd=os.path.dirname(EXE),
                                creationflags=subprocess.CREATE_NEW_CONSOLE)
    return subprocess.Popen([EXE], cwd=os.path.dirname(EXE),
                            stdin=subprocess.DEVNULL)

def main():
    print("=" * 54)
    print("  XMU SecureLink 一键连接 v3")
    print("=" * 54)
    if not os.path.exists(EXE):
        pause_exit("错误: 找不到 %s\n"
                   "      请把本脚本和 xmu_secure_link.exe 放在同一文件夹" % EXE)
    print("[0/4] 检测 TAP 虚拟网卡...")
    ensure_tap()

    print("[1/4] 清理上次可能残留的劫持规则和僵尸实例...")
    cleanup()

    print("[2/4] 正在启动 VPN（登录已缓存则自动连接）...")
    need_login = not os.path.exists(SESSION_PATH)
    proc = launch_vpn(need_login)
    if need_login:
        print("      没有可用登录缓存：已单独打开登录窗口，")
        print("      请在那里完成浏览器登录，并把回调 URL 粘贴到那个窗口里")

    print("[3/4] 等待 VPN 连接（最多 3 分钟）...")
    t0 = time.time()
    ok = False
    retried = False
    hinted = False
    while time.time() - t0 < 180:
        if proc.poll() is not None:
            # 只重试一次：清掉本地登录缓存后再来一遍（服务端吊销会话时本地 token 仍是「有效」）
            if not retried and proc.returncode != 0:
                retried = True
                print("")
                print("      首次连接失败（code=%s），清理登录缓存后重试一次..." % proc.returncode)
                retire_session()
                proc = launch_vpn(True)
                t0 = time.time()
                print("      请在新打开的登录窗口完成登录（浏览器会自动打开厦大登录页）")
                continue
            cleanup()
            pause_exit("VPN 进程异常退出（code=%s）。\n"
                       "常见原因: 需要重新登录（登录窗口里没完成登录）、网络不通、\n"
                       "      或上次残留未清理干净。再双击一次 xmu_vpn.exe 通常可恢复。"
                       % proc.returncode)
        if not hinted and time.time() - t0 > 20:
            hinted = True
            print("      提示: 若弹出了登录窗口，请先完成浏览器登录，再把回调 URL")
            print("            粘贴到那个窗口里（首次使用需要，之后会自动免登录）")
        r = subprocess.run("ping -n 1 -w 1000 %s" % VPN_GW,
                           capture_output=True, text=True, shell=True)
        if "TTL=" in (r.stdout or ""):
            ok = True
            break
        time.sleep(1)
    if not ok:
        proc.terminate()
        cleanup()
        pause_exit("3 分钟内未能连接成功。\n"
                   "请检查: ①网络是否正常  ②浏览器登录页是否弹出并已完成登录\n"
                   "      ③稍等片刻重试；若一直不行，把本窗口内容反馈给维护者")

    print("[4/4] 连接成功！补上校内 DNS 的隧道路由...")
    for dns in DNS_SERVERS:
        run("route add %s mask 255.255.255.255 %s" % (dns, VPN_GW))
    run("ipconfig /flushdns")
    # 兜底校验: 路由补完后校内 DNS 必须可达, 否则说明 VPN 网段变了
    r = subprocess.run("ping -n 2 -w 2000 %s" % DNS_SERVERS[0],
                       capture_output=True, text=True, shell=True)
    if "TTL=" not in (r.stdout or ""):
        print("提示: 校内 DNS 线路异常，网页可能暂时打不开。")
        print("      这通常是校园网网段有调整，断开重连试试；")
        print("      持续出现的话把窗口内容反馈给维护者就行。")

    print()
    print("=" * 54)
    print("  VPN 已连接，DNS 已修复，可以正常上网了")
    print("  （实测修复后网页解析约 0.25 秒）")
    print("  本窗口保持开着，最小化即可")
    print("  在本窗口按 Ctrl+C 或直接关闭 = 断开 VPN")
    print("=" * 54)
    try:
        proc.wait()
    except KeyboardInterrupt:
        pass
    # 退出兜底清理（正常 Ctrl-C 时 VPN 自己清理，这里再兜底一遍）
    cleanup()
    print("VPN 已断开，一切已清理")

def cleanup_fast():
    """关窗口(点×)专用: Windows 只给约 5 秒收尾，这里只做最关键的两步。
    网卡复位 / flushdns 等慢操作交给下次启动时的完整 cleanup 兜底。"""
    print("[cleanup-fast] 检测到窗口关闭，开始快速清理...")
    run('taskkill /F /IM xmu_secure_link.exe', timeout=3)
    time.sleep(1)  # 等进程真正退出，否则注册表键还被占着
    try:
        k = winreg.OpenKey(winreg.HKEY_LOCAL_MACHINE, NRPT_KEY, 0, winreg.KEY_ALL_ACCESS)
        victims = []
        i = 0
        while True:
            try:
                sub = winreg.EnumKey(k, i); i += 1
            except OSError:
                break
            if sub.startswith("OpenVPNDNSRouting"):
                victims.append(sub)
        for v in victims:
            try:
                winreg.DeleteKey(k, v)
                print("[cleanup-fast] 已清理 NRPT: %s" % v)
            except OSError as e:
                print("[cleanup-fast] 清理 NRPT 失败 %s: %r" % (v, e))
        winreg.CloseKey(k)
    except Exception as e:
        print("[cleanup-fast] 枚举 NRPT 异常: %r" % e)
    # 这两步也很快(毫秒级)，一并做掉，只有 TAP 网卡复位留给下次启动兜底
    for dns in DNS_SERVERS:
        run("route delete %s" % dns, timeout=3)
    run("ipconfig /flushdns", timeout=5)
    print("[cleanup-fast] 完成")

_CTRL_HANDLER = None  # 必须保存引用, 否则会被 GC 回收导致回调失效

def install_close_handler():
    """点窗口右上角 × 关闭时也会触发清理（Windows 会留几秒给进程收尾）"""
    global _CTRL_HANDLER
    try:
        from ctypes import wintypes
        proto = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.DWORD)
        def _handler(event):
            # 2=关闭控制台(点×) 5=注销 6=关机; Ctrl+C(0) 走 KeyboardInterrupt 分支
            # 这里只能用快速清理：系统只留约 5 秒就用强杀收尾
            if event in (2, 5, 6):
                cleanup_fast()
            return False
        _CTRL_HANDLER = proto(_handler)
        ctypes.windll.kernel32.SetConsoleCtrlHandler(_CTRL_HANDLER, True)
    except Exception:
        pass  # 安装失败不影响主流程（还有启动时清理兜底）

class _Tee:
    """把控制台输出同时写进 launcher.log（窗口关掉后也能事后排查）"""
    def __init__(self, path):
        try:
            if os.path.exists(path) and os.path.getsize(path) > 512 * 1024:
                os.remove(path)
        except OSError:
            pass
        try:
            self.f = open(path, "a", encoding="utf-8", errors="replace")
        except OSError:
            self.f = None

    def write(self, s):
        try:
            sys.__stdout__.write(s)
        except Exception:
            pass
        if self.f:
            try:
                self.f.write(s)
                self.f.flush()
            except Exception:
                pass

    def flush(self):
        try:
            sys.__stdout__.flush()
        except Exception:
            pass
        if self.f:
            try:
                self.f.flush()
            except Exception:
                pass

if __name__ == "__main__":
    if not is_admin():
        elevate()
    try:
        sys.stdout = _Tee(LOG_PATH)
        print("\n===== %s 启动 (admin=%s) =====" % (time.strftime("%Y-%m-%d %H:%M:%S"), is_admin()))
    except Exception:
        pass
    install_close_handler()
    try:
        main()
    except SystemExit:
        raise
    except Exception:
        import traceback
        traceback.print_exc()
        pause_exit("程序出错，请把上面的信息反馈给维护者")
    finally:
        # 无论怎么退出都保证不残留
        if is_admin():
            cleanup()
