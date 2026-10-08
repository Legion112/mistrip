#!/usr/bin/env python3
"""Fetch miIO device tokens from your Mi Cloud account.

Your password is read with getpass: it is never echoed, never stored in shell
history, and never passed on the command line. Tokens are written to a
0600 config file rather than printed to the terminal.

Run:  ~/.venvs/miio/bin/python get-token.py
"""
import getpass
import os
import stat
import sys
from pathlib import Path

from miio.cloud import CloudException, CloudInterface

CONFIG = Path.home() / ".config" / "mistrip" / "devices.toml"
INTERESTING = ("light.strip", "light.", "lightstrip")


def mask(token: str) -> str:
    if not token or len(token) < 8:
        return "<empty>"
    return f"{token[:4]}{'*' * (len(token) - 8)}{token[-4:]}"


def toml_escape(value: str) -> str:
    return str(value).replace("\\", "\\\\").replace('"', '\\"')


def main() -> int:
    print("Mi Cloud login — password is not echoed and not stored in history.\n")
    username = input("Mi account (email / phone / Mi ID): ").strip()
    if not username:
        print("no username given", file=sys.stderr)
        return 1
    password = getpass.getpass("Password: ")
    if not password:
        print("no password given", file=sys.stderr)
        return 1

    print("\nLogging in and browsing all server regions (cn, de, i2, ru, sg, us)...")
    try:
        devices = CloudInterface(username=username, password=password).get_devices()
    except CloudException as exc:
        print(f"\nLogin failed: {exc}", file=sys.stderr)
        print(
            "If your account has 2FA enabled, micloud cannot complete the login "
            "non-interactively. See python-miio's legacy token extraction docs for "
            "alternatives (ADB backup / rooted Mi Home database).",
            file=sys.stderr,
        )
        return 2
    except Exception as exc:  # noqa: BLE001 - surface whatever micloud raised
        print(f"\nUnexpected error from micloud: {exc!r}", file=sys.stderr)
        return 2
    finally:
        del password

    if not devices:
        print("\nLogin succeeded but the account has no devices in any region.")
        print("Check that the strip is added in Mi Home under this same account.")
        return 3

    print(f"\nFound {len(devices)} device(s):\n")
    print(f"  {'model':<28} {'name':<22} {'ip':<16} {'region':<8} token")
    print(f"  {'-' * 28} {'-' * 22} {'-' * 16} {'-' * 8} {'-' * 20}")
    lights = []
    for dev in sorted(devices.values(), key=lambda d: d.model):
        region = ",".join(dev.locale)
        print(
            f"  {dev.model:<28} {dev.name[:22]:<22} {dev.ip or '-':<16} "
            f"{region:<8} {mask(dev.token)}"
        )
        if any(key in dev.model for key in INTERESTING):
            lights.append(dev)

    CONFIG.parent.mkdir(parents=True, exist_ok=True)
    with open(CONFIG, "w", encoding="utf-8") as fh:
        os.chmod(CONFIG, stat.S_IRUSR | stat.S_IWUSR)
        fh.write("# miIO devices from Mi Cloud. Contains secrets — keep 0600.\n")
        for dev in sorted(devices.values(), key=lambda d: d.model):
            fh.write(f'\n[devices."{toml_escape(dev.did)}"]\n')
            fh.write(f'model = "{toml_escape(dev.model)}"\n')
            fh.write(f'name = "{toml_escape(dev.name)}"\n')
            fh.write(f'ip = "{toml_escape(dev.ip)}"\n')
            fh.write(f'token = "{toml_escape(dev.token)}"\n')
            fh.write(f'mac = "{toml_escape(dev.mac)}"\n')
            fh.write(f'region = "{toml_escape(",".join(dev.locale))}"\n')
    os.chmod(CONFIG, stat.S_IRUSR | stat.S_IWUSR)

    print(f"\nFull details incl. tokens written to {CONFIG} (mode 0600)")
    if lights:
        print("\nLight devices found:")
        for dev in lights:
            print(f"  {dev.model}  ip={dev.ip or 'unknown'}  did={dev.did}")
    else:
        print("\nNo 'light.*' model in the account — the strip may be under "
              "another account or not yet added in Mi Home.")
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except KeyboardInterrupt:
        print()
        sys.exit(130)
