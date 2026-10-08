#!/usr/bin/env python3
"""Read-only probe of the Xiaomi Smart Lightstrip Pro (philips.light.strip5).

Reads ip/token from ~/.config/mistrip/devices.json so no secret touches argv.
Only reads properties — changes nothing.
"""
import json
import sys
from pathlib import Path

from miio import MiotDevice

CONFIG = Path.home() / ".config" / "mistrip" / "devices.json"
MODEL = "philips.light.strip5"

# (siid, piid, label) from the published miot spec
READABLE = [
    (2, 1, "on"),
    (2, 2, "mode"),
    (2, 3, "brightness"),
    (2, 4, "color"),
    (3, 2, "mitv-rhythm"),
    (3, 3, "acousto-optic-rhythm"),
    (3, 4, "dvalue (sleep timer)"),
    (3, 6, "rhythm-color-type"),
    (3, 7, "rhythm-sensitivity"),
    (3, 8, "rhythm-animation"),
    (3, 12, "diy-id"),
    (3, 15, "mitv-available"),
    (3, 16, "length-strip"),
    (3, 19, "diy-free-id"),
    (3, 20, "control-scene"),
]


def find_device():
    for server in json.load(open(CONFIG)):
        for home in server.get("homes", []):
            for dev in home.get("devices", []):
                if dev.get("model") == MODEL:
                    return dev
    return None


def main() -> int:
    dev = find_device()
    if not dev:
        print(f"{MODEL} not found in {CONFIG}", file=sys.stderr)
        return 1
    ip, token = dev["localip"], dev["token"]
    print(f"{dev['name']}  {MODEL}  {ip}  did={dev['did']}\n")

    d = MiotDevice(ip=ip, token=token)

    try:
        info = d.info()
        print(f"  firmware : {info.firmware_version}")
        print(f"  hardware : {info.hardware_version}")
        print(f"  model    : {info.model}")
        print(f"  mac      : {info.mac_address}")
    except Exception as exc:  # noqa: BLE001
        print(f"  info() failed: {exc!r}")
        return 2

    print("\n  properties:")
    for siid, piid, label in READABLE:
        try:
            res = d.get_property_by(siid, piid)
            entry = res[0] if isinstance(res, list) and res else res
            code = entry.get("code") if isinstance(entry, dict) else None
            value = entry.get("value") if isinstance(entry, dict) else entry
            status = "" if code == 0 else f"  (code={code})"
            print(f"    {siid}.{piid:<3} {label:<24} = {value!r}{status}")
        except Exception as exc:  # noqa: BLE001
            print(f"    {siid}.{piid:<3} {label:<24} ! {exc!r}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
