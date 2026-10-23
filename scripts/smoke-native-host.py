#!/usr/bin/env python3
"""Read-only native host smoke; deliberately NOT installation acceptance."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import selectors
import subprocess
import tempfile
import time
import urllib.request


def smoke(binary, version, revision, binary_sha256):
    os_release = platform.freedesktop_os_release()
    if (os_release.get("ID"), os_release.get("VERSION_ID"), platform.machine()) != (
        "ubuntu", "22.04", "x86_64"
    ):
        raise RuntimeError("Requires native Ubuntu 22.04 AMD64")
    if hashlib.sha256(binary.read_bytes()).hexdigest() != binary_sha256:
        raise RuntimeError("Binary digest differs from CI provenance")
    info = json.loads(subprocess.check_output([binary, "--build-info"], timeout=10))
    if (info.get("version"), info.get("revision"), info.get("clean"), info.get("platform"), info.get("architecture")) != (
        version, revision, True, "linux", "x86_64"
    ):
        raise RuntimeError("Binary build identity differs from CI provenance")
    if subprocess.check_output([binary, "--version"], timeout=10).decode().strip() != version:
        raise RuntimeError("Version output mismatch")
    with tempfile.TemporaryDirectory(prefix="apeiron-native-smoke-") as directory:
        env = {"HOME": directory, "PATH": "/usr/bin:/bin:/usr/sbin:/sbin"}
        child = subprocess.Popen(
            [binary, "init", "--no-open", "--config", str(Path(directory) / "config.json")],
            env=env, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
        )
        selector = selectors.DefaultSelector()
        selector.register(child.stdout, selectors.EVENT_READ)
        try:
            output = b""
            deadline = time.monotonic() + 30
            url = None
            while time.monotonic() < deadline:
                if child.poll() is not None:
                    raise RuntimeError("Wizard exited before readiness")
                for key, _ in selector.select(0.25):
                    output += os.read(key.fileobj.fileno(), 8192)
                    match = re.search(rb"url\t(http://127\.0\.0\.1:\d+/setup/[a-f0-9]+/)", output)
                    if match:
                        url = match[1].decode()
                        break
                if url:
                    break
            if not url:
                raise RuntimeError("Wizard readiness timed out")
            # Never print the capability URL or the full configuration response.
            opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
            with opener.open(url, timeout=10) as response:
                if response.status != 200 or b"favicon.ico" not in response.read():
                    raise RuntimeError("Wizard page failed")
            with opener.open(url + "api/config", timeout=10) as response:
                config = json.load(response)
            with opener.open(url + "api/deployment", timeout=10) as response:
                deployment = json.load(response)
            if config["deployment"]["phase"] != "idle" or deployment["phase"] != "idle":
                raise RuntimeError("Fresh wizard is not idle")
            return {
                "schemaVersion": 1,
                "kind": "native-host-smoke",
                "version": version,
                "revision": revision,
                "binarySha256": binary_sha256,
                "os": "ubuntu:22.04",
                "arch": "amd64",
                "checks": {"version": True, "buildInfo": True, "wizard": True, "configApi": True},
                "installationTested": False,
            }
        finally:
            selector.close()
            child.terminate()
            try:
                child.wait(timeout=10)
            except subprocess.TimeoutExpired:
                child.kill()
                child.wait()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--version", required=True)
    parser.add_argument("--revision", required=True)
    parser.add_argument("--binary-sha256", required=True)
    args = parser.parse_args()
    print(json.dumps(smoke(args.binary.resolve(), args.version, args.revision, args.binary_sha256), indent=2))
