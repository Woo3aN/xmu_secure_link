# -*- coding: utf-8 -*-
# 网络修复（急救工具）: 一键清除 VPN 异常退出留下的 DNS 劫持/网卡残留
# 用途: 因为各种原因（直接关窗口、强杀进程、系统崩溃）导致"能连网但网页打不开/很卡"时
#       双击同目录的 网络修复.cmd 即可，无需懂任何原理。
import ctypes, os, sys, subprocess, winreg, time

BASE = os.path.dirname(os.path.abspath(__file__))
NRPT = r"SYSTEM\CurrentControlSet\Services\Dnscache\Parameters\DnsPolicyConfig"

def is_admin():
    try:
        return bool(ctypes.windll.shell32.IsUserAnAdmin())
    except Exception:
        return False

def run(cmd, timeout=90):
    try:
        r = subprocess.run(cmd, shell=True, capture_output=True, timeout=timeout)
        return ((r.stdout or b"") + (r.stderr or b"")).decode("gbk", errors="replace").strip()
    except Exception as e:
        return repr(e)

TAP_RESET_PS = ("Get-NetAdapter | Where-Object { $_.InterfaceDescription -match 'TAP-Windows' } | "
                "ForEach-Object { $i = $_.ifIndex; "
                "Get-NetIPAddress -InterfaceIndex $i -AddressFamily IPv4 -ErrorAction SilentlyContinue | "
                "Where-Object { $_.PrefixOrigin -eq 'Manual' } | "
                "Remove-NetIPAddress -Confirm:$false -ErrorAction SilentlyContinue; "
                "Set-DnsClientServerAddress -InterfaceIndex $i -ResetServerAddresses -ErrorAction SilentlyContinue }")

def main():
    print("=" * 54)
    print("  网络修复（清除 VPN 残留，恢复正常上网）")
    print("=" * 54)

    print("[1/4] 结束残留的 VPN 进程...")
    run('taskkill /F /IM xmu_secure_link.exe')  # 无进程时静默
    time.sleep(2)  # 等系统释放仍被占用的策略/网卡配置，否则后面删除会被拒绝
    print("      完成")

    print("[2/4] 清除 DNS 劫持规则...")
    n = 0
    failed = []
    try:
        k = winreg.OpenKey(winreg.HKEY_LOCAL_MACHINE, NRPT, 0, winreg.KEY_ALL_ACCESS)
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
                    n += 1
                    break
                except OSError as e:
                    if attempt == 2:
                        failed.append((v, repr(e)))
                    else:
                        time.sleep(1)
        winreg.CloseKey(k)
    except Exception as e:
        print("      读取规则失败: %r" % e)
    print("      已删除 %d 条劫持规则" % n)
    for v, err in failed:
        print("      删除失败 %s: %s" % (v, err))

    print("[3/4] 复位 TAP 虚拟网卡（清除残留的校内 DNS / IP）...")
    run('powershell -NoProfile -Command "%s"' % TAP_RESET_PS)
    print("      完成")

    print("[4/4] 刷新 DNS 缓存...")
    run("ipconfig /flushdns")

    print()
    ms = run('powershell -NoProfile -Command '
             '"(Measure-Command { Resolve-DnsName www.baidu.com -ErrorAction SilentlyContinue | Out-Null }).TotalMilliseconds"')
    print("自检: DNS 解析耗时 %s ms（正常应小于 500ms）" % ms)
    print()
    print("修复完成！现在打开网页试试，应该正常了。")

if __name__ == "__main__":
    if not is_admin():
        print("需要管理员权限，正在请求提权（请在 UAC 弹窗点\"是\"）...")
        ret = ctypes.windll.shell32.ShellExecuteW(None, "runas", sys.executable,
                                                  '"%s"' % os.path.abspath(__file__), None, 1)
        if ret == 1223:
            print("已取消提权，未做任何修改。")
        elif ret <= 32:
            print("提权失败（错误码 %s）" % ret)
        try:
            input("\n按回车键退出...")
        except EOFError:
            pass
        sys.exit(0)
    main()
    try:
        input("\n按回车键退出...")
    except Exception:
        pass
    sys.exit(0)
