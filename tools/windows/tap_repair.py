# -*- coding: utf-8 -*-
# TAP 网卡修复工具（便携版）——给 xmu_secure_link 创建它必需的 TAP-Windows 虚拟网卡
# 用法: 右键本文件 → "使用管理员运行"（或让 vpn.cmd 在检测缺失时引导运行）
# 原理: 等价于官方 devcon install —— 从系统驱动库里找 tap0901 驱动包，
#       按 devcon 官方实现（SetupDiGetINFClass → CreateDeviceInfo → DIF_REGISTERDEVICE
#       → UpdateDriverForPlugAndPlayDevices）创建 TAP 设备实例并装上驱动。
import ctypes, subprocess, os, sys, re, winreg
from ctypes import wintypes

setupapi = ctypes.WinDLL("setupapi.dll", use_last_error=True)
newdev = ctypes.WinDLL("newdev.dll", use_last_error=True)

class GUID(ctypes.Structure):
    _fields_ = [("Data1", wintypes.DWORD),
                ("Data2", wintypes.WORD),
                ("Data3", wintypes.WORD),
                ("Data4", ctypes.c_ubyte * 8)]

class SP_DEVINFO_DATA(ctypes.Structure):
    _fields_ = [("cbSize", wintypes.DWORD),
                ("ClassGuid", GUID),
                ("DevInst", wintypes.DWORD),
                ("Reserved", ctypes.POINTER(ctypes.c_ulong))]

INVALID_HANDLE_VALUE = ctypes.c_void_p(-1).value
DICD_GENERATE_ID = 0x00000001
SPDRP_HARDWAREID = 0x00000001
DIF_REGISTERDEVICE = 0x00000019
INSTALLFLAG_FORCE = 0x00000001

setupapi.SetupDiGetINFClassW.argtypes = [wintypes.LPCWSTR, ctypes.POINTER(GUID),
                                         wintypes.LPWSTR, wintypes.DWORD, ctypes.POINTER(wintypes.DWORD)]
setupapi.SetupDiGetINFClassW.restype = wintypes.BOOL
# 无字符串参数的 API 没有 A/W 之分，导出名不带后缀，补别名
setupapi.SetupDiCreateDeviceInfoListW = setupapi.SetupDiCreateDeviceInfoList
setupapi.SetupDiCreateDeviceInfoListW.argtypes = [ctypes.POINTER(GUID), wintypes.HWND]
setupapi.SetupDiCreateDeviceInfoListW.restype = wintypes.HANDLE
setupapi.SetupDiCreateDeviceInfoW.argtypes = [wintypes.HANDLE, wintypes.LPCWSTR,
                                              ctypes.POINTER(GUID), wintypes.LPCWSTR,
                                              wintypes.HWND, wintypes.DWORD,
                                              ctypes.POINTER(SP_DEVINFO_DATA)]
setupapi.SetupDiCreateDeviceInfoW.restype = wintypes.BOOL
setupapi.SetupDiSetDeviceRegistryPropertyW.argtypes = [wintypes.HANDLE,
                                                       ctypes.POINTER(SP_DEVINFO_DATA),
                                                       wintypes.DWORD, wintypes.LPCWSTR, wintypes.DWORD]
setupapi.SetupDiSetDeviceRegistryPropertyW.restype = wintypes.BOOL
setupapi.SetupDiCallClassInstaller.argtypes = [wintypes.DWORD, wintypes.HANDLE,
                                               ctypes.POINTER(SP_DEVINFO_DATA)]
setupapi.SetupDiCallClassInstaller.restype = wintypes.BOOL
newdev.UpdateDriverForPlugAndPlayDevicesW.argtypes = [wintypes.HWND, wintypes.LPCWSTR,
                                                      wintypes.LPCWSTR, wintypes.DWORD,
                                                      ctypes.POINTER(wintypes.BOOL)]
newdev.UpdateDriverForPlugAndPlayDevicesW.restype = wintypes.BOOL

def is_admin():
    try:
        return bool(ctypes.windll.shell32.IsUserAnAdmin())
    except Exception:
        return False

def find_tap_inf():
    """定位 tap0901 驱动 INF：
    ① 分发包自带的 tapdriver\（无需任何官方软件，推荐路径）
    ② 系统驱动库（本机装过官方 SecureLink / OpenVPN 时可用）"""
    # ① 分发包自带
    # 打包成 exe 时以 exe 所在目录为准（PyInstaller 下 __file__ 指向临时目录）
    if getattr(sys, "frozen", False):
        base_dir = os.path.dirname(os.path.abspath(sys.executable))
    else:
        base_dir = os.path.dirname(os.path.abspath(__file__))
    local = os.path.join(base_dir, "tapdriver")
    if os.path.isdir(local):
        for f in os.listdir(local):
            if f.lower().endswith(".inf"):
                path = os.path.join(local, f)
                try:
                    if "tap0901" in open(path, encoding="gbk", errors="replace").read().lower():
                        return path
                except OSError:
                    pass
    # ② 系统驱动库
    try:
        r = subprocess.run(["pnputil", "/enum-drivers"], capture_output=True, timeout=60)
    except Exception:
        return None
    # 中文系统输出是 GBK；关键匹配串都是 ASCII，errors=replace 保平安
    text = r.stdout.decode("gbk", errors="replace") + r.stderr.decode("gbk", errors="replace")
    for block in re.split(r"\n\s*\n", text):
        if "tap0901" in block.lower():
            m = re.search(r"(oem\d+\.inf)", block, re.I)
            if m:
                return os.path.join(r"C:\Windows\INF", m.group(1))
    return None

def install(inf_path):
    hwid = "tap0901"
    class_guid = GUID()
    class_name = ctypes.create_unicode_buffer(256)
    if not setupapi.SetupDiGetINFClassW(inf_path, ctypes.byref(class_guid),
                                        class_name, 256, None):
        raise OSError("SetupDiGetINFClass 失败 err=0x%08X" % ctypes.get_last_error())
    h = setupapi.SetupDiCreateDeviceInfoListW(ctypes.byref(class_guid), None)
    if h in (None, INVALID_HANDLE_VALUE):
        raise OSError("SetupDiCreateDeviceInfoList 失败")
    did = SP_DEVINFO_DATA()
    did.cbSize = ctypes.sizeof(SP_DEVINFO_DATA)
    if not setupapi.SetupDiCreateDeviceInfoW(h, class_name.value, ctypes.byref(class_guid),
                                             None, None, DICD_GENERATE_ID, ctypes.byref(did)):
        raise OSError("SetupDiCreateDeviceInfo 失败 err=0x%08X" % ctypes.get_last_error())
    buf = ctypes.create_unicode_buffer(hwid + "\0")
    if not setupapi.SetupDiSetDeviceRegistryPropertyW(h, ctypes.byref(did), SPDRP_HARDWAREID,
                                                      buf, ctypes.sizeof(buf)):
        raise OSError("SetDeviceRegistryProperty(HARDWAREID) 失败")
    if not setupapi.SetupDiCallClassInstaller(DIF_REGISTERDEVICE, h, ctypes.byref(did)):
        raise OSError("DIF_REGISTERDEVICE 失败 err=0x%08X" % ctypes.get_last_error())
    reboot = wintypes.BOOL(False)
    if not newdev.UpdateDriverForPlugAndPlayDevicesW(None, hwid, inf_path,
                                                     INSTALLFLAG_FORCE, ctypes.byref(reboot)):
        raise OSError("UpdateDriverForPlugAndPlayDevices 失败 err=0x%08X" % ctypes.get_last_error())
    return bool(reboot.value)

def tap_present():
    """系统里是否已有 TAP-Windows 适配器（避免重复创建）"""
    net_class = r"SYSTEM\CurrentControlSet\Control\Class\{4D36E972-E325-11CE-BFC1-08002BE10318}"
    try:
        k = winreg.OpenKey(winreg.HKEY_LOCAL_MACHINE, net_class, 0, winreg.KEY_READ)
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

def main():
    print("=" * 54)
    print("  TAP 虚拟网卡修复工具（XMU SecureLink 用）")
    print("=" * 54)
    if tap_present():
        print("系统里已有 TAP-Windows 虚拟网卡，无需重复创建。")
        print("直接双击 vpn.cmd 连接 VPN 即可。")
        return 0
    inf = find_tap_inf()
    if not inf:
        print("分发包和系统里都没有找到 TAP 驱动（tap0901）。")
        print("正常情况下本文件夹自带 tapdriver 子目录（含驱动），")
        print("请重新完整拷贝整个文件夹后再试；")
        print("或者安装一次 OpenVPN 官方安装包（勾选 TAP 组件）后重试。")
        return 1
    print("找到 TAP 驱动: %s" % inf)
    try:
        need_reboot = install(inf)
        print("完成！TAP-Windows 虚拟网卡已创建%s" %
              ("（需要重启电脑）" if need_reboot else "，无需重启"))
        print("现在可以双击 vpn.cmd 连接 VPN 了")
        return 0
    except OSError as e:
        print("失败: %s" % e)
        print("如果错误是权限类（拒绝访问），请确认是以管理员身份运行")
        return 1

class _Tee:
    """同时写控制台和结果文件（提权窗口的输出外部读不到，落盘备查）"""
    def __init__(self, path):
        self.f = open(path, "w", encoding="utf-8", errors="replace")
    def write(self, s):
        sys.__stdout__.write(s)
        self.f.write(s)
    def flush(self):
        sys.__stdout__.flush()
        self.f.flush()

if __name__ == "__main__":
    sys.stdout = _Tee(os.path.join(os.path.dirname(os.path.abspath(__file__)),
                                   "tap_repair_result.txt"))
    if not is_admin():
        print("需要管理员权限，正在请求提权（请在 UAC 弹窗点\"是\"）...")
        ret = ctypes.windll.shell32.ShellExecuteW(None, "runas", sys.executable,
                                                  '"%s"' % os.path.abspath(__file__), None, 1)
        if ret == 1223:
            print("已取消提权（UAC 弹窗需要点\"是\"才能继续）")
        elif ret <= 32:
            print("提权失败（错误码 %s）" % ret)
        else:
            print("已授权，管理员窗口应该已经出现；如果没看到新窗口，")
            print("请到任务栏找一下，或重新双击本文件")
        input("\n按回车键退出...")
        sys.exit(0)
    code = main()
    try:
        input("\n按回车键退出...")
    except EOFError:
        pass  # 无控制台 stdin 时（被其他程序调用）直接退出
    sys.exit(code)
