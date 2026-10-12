# -*- coding: utf-8 -*-
"""检查录音卡是否具备 WiFi 功能。

原理：WiFi 设备首次配网几乎都要走 BLE 通道（设备开热点/AP 模式除外），
所以看两点：
  1. 广播包里有没有 WiFi 配网相关的 Service UUID；
  2. 连上后枚举 GATT 服务，有没有 WiFi provisioning 服务。
如果两者皆无、只剩厂商自定义服务，基本可判定这张卡没有 WiFi（纯蓝牙笔）。

用法（用仓库的 pnote 虚拟环境跑，已装 bleak）：
    .venv-pnote/Scripts/python.exe tools/check_card_wifi.py [广播名，默认 cb08]

注意：App 的录音卡守护正在连接时，设备会停止广播，扫不到——
请先退出 App 或点浏览页的「断开连接」，再按一下录音卡按键唤醒。
"""
import asyncio
import sys
import time

# 常见 WiFi 配网/相关 BLE 服务（小写 UUID → 说明）
WIFI_SERVICE_HINTS = {
    "0000fe2c-0000-1000-8000-00805f9b34fb": "Amazon Wi-Fi Simple Setup（配网）",
    "0000fef5-0000-1000-8000-00805f9b34fb": "Wi-Fi Easy Connect / DPP（配网）",
    "4fafc201-1fb5-459e-8fcc-c5c9c331914b": "ESP32 WiFi Provisioning（配网）",
    "0000ff00-0000-1000-8000-00805f9b34fb": "厂商配网服务（ESP 系常见，疑似 WiFi 配网）",
    "0000fff0-0000-1000-8000-00805f9b34fb": "厂商自定义服务（部分用于 WiFi 配网）",
}

KNOWN_SERVICES = {
    "00001800-0000-1000-8000-00805f9b34fb": "Generic Access（必有）",
    "00001801-0000-1000-8000-00805f9b34fb": "Generic Attribute（必有）",
    "0000180a-0000-1000-8000-00805f9b34fb": "Device Information（设备信息）",
    "0000180f-0000-1000-8000-00805f9b34fb": "Battery Service（电量）",
}


async def main() -> int:
    # Windows 控制台默认 GBK，强制 UTF-8 输出
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    sys.stderr.reconfigure(encoding="utf-8", errors="replace")
    name = sys.argv[1] if len(sys.argv) > 1 else "cb08"
    print(f"扫描广播名 {name!r} 的设备…（30 秒，请确认录音卡已开机、在电脑旁，且 App 未占用）")
    holder: dict = {}
    adv_uuids: list[str] = []

    def cb(d, ad):
        local = (ad.local_name or d.name or "").lower()
        if local != name.lower():
            return
        holder["device"] = d
        holder["at"] = time.monotonic()
        # bleak 3.x：广播里携带的 service uuid 列表
        uuids = getattr(ad, "service_uuids", None) or []
        if uuids and not adv_uuids:
            adv_uuids.extend(u.lower() for u in uuids)

    scanner_cb = cb
    from bleak import BleakScanner

    scanner = BleakScanner(detection_callback=scanner_cb)
    await scanner.start()
    deadline = time.monotonic() + 30
    found = False
    try:
        while time.monotonic() < deadline:
            if holder and time.monotonic() - holder["at"] <= 3:
                found = True
                break
            await asyncio.sleep(0.5)
    finally:
        await scanner.stop()

    if not found:
        print("✗ 30 秒内没有扫到设备。可能：")
        print("  - App 正连着这张卡（设备停止广播）→ 退出 App 或点「断开连接」再试")
        print("  - 录音卡休眠了 → 按一下卡上的按键唤醒")
        print("  - 不在蓝牙范围内 → 把卡放到电脑旁边")
        return 1

    device = holder["device"]
    print(f"✓ 发现设备 {device.name} ({device.address})")
    if adv_uuids:
        print(f"  广播携带的服务 UUID：{len(adv_uuids)} 个")
        hit = [u for u in adv_uuids if u in WIFI_SERVICE_HINTS]
        for u in adv_uuids:
            label = WIFI_SERVICE_HINTS.get(u) or KNOWN_SERVICES.get(u) or "未知/厂商自定义"
            mark = "  ← WiFi 相关！" if u in WIFI_SERVICE_HINTS else ""
            print(f"    {u}  {label}{mark}")
        if hit:
            print("→ 广播里就有 WiFi 配网服务：这张卡大概率支持 WiFi")
            return 0
    else:
        print("  广播未携带服务 UUID（常见于小数据量设备，不代表无 WiFi）")

    print("\n连接并枚举 GATT 服务…")
    from bleak import BleakClient

    async with BleakClient(device, timeout=30) as client:
        services = list(client.services)
        wifi_hit = []
        print(f"  共发现 {len(services)} 个服务：")
        for svc in services:
            u = svc.uuid.lower()
            if u in WIFI_SERVICE_HINTS:
                wifi_hit.append(u)
            label = WIFI_SERVICE_HINTS.get(u) or KNOWN_SERVICES.get(u) or "厂商自定义"
            print(f"    {u}  {label}")
    if wifi_hit:
        print("\n→ 发现 WiFi 配网服务：这张卡支持 WiFi 的可能性很高")
        return 0
    print("\n→ 未发现任何 WiFi 配网服务，且只有蓝牙直连服务：")
    print("  基本可以判定这张卡是「纯蓝牙」录音卡，没有 WiFi 功能。")
    print("  （若厂商用 AP 热点方式配网则不经过 BLE，这种情况看说明书/配套 App 是否有云同步可确认。）")
    return 0


if __name__ == "__main__":
    sys.exit(asyncio.run(main()))
