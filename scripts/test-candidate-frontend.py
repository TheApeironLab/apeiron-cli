#!/usr/bin/env python3
"""Verify the actual frontend image can serve two independent installation sites."""
import hashlib
import json
import pathlib
import re
import subprocess
import sys
import time
import urllib.request

image, output = sys.argv[1:]
receipts = []
for site in ("candidate-a", "candidate-b"):
    issuer = f"https://id.{site}.example/realms/master"
    client = f"apeiron-{site}"
    container = subprocess.check_output([
        "docker", "run", "-d", "-p", "127.0.0.1::8080",
        "-e", f"APEIRON_OIDC_ISSUER={issuer}",
        "-e", f"APEIRON_OIDC_CLIENT_ID={client}",
        "-e", "APEIRON_OIDC_CLIENT_SECRET=must-not-be-exposed", image,
    ], text=True).strip()
    try:
        state = json.loads(subprocess.check_output(["docker", "inspect", container]))[0]
        port = state["NetworkSettings"]["Ports"]["8080/tcp"][0]["HostPort"]
        base = f"http://127.0.0.1:{port}"
        for attempt in range(60):
            try:
                response = urllib.request.urlopen(base + "/apeiron-runtime-config.js", timeout=2)
                break
            except OSError:
                if attempt == 59:
                    raise
                time.sleep(0.5)
        with response:
            script = response.read().decode()
            assert response.headers.get_content_type() == "application/javascript"
            assert response.headers.get("Cache-Control") == "no-store"
            prefix = "globalThis.__APEIRON_RUNTIME_CONFIG__ = "
            assert script.startswith(prefix) and "must-not-be-exposed" not in script
            config = json.loads(script[len(prefix):].strip().removesuffix(";"))
            assert config == {"oidc": {"issuer": issuer, "clientId": client}}
        with urllib.request.urlopen(base + "/", timeout=5) as response:
            html = response.read().decode()
        assert "apeiron-runtime-config.js" in html
        assets = re.findall(r'<script[^>]*src="([^"]+\.js)"', html)
        assets = [p for p in assets if "apeiron-runtime-config.js" not in p]
        assert assets, "No built application JavaScript entry"
        hashes = {}
        for path in assets:
            assert path.startswith("/") and not path.startswith("//")
            with urllib.request.urlopen(base + path, timeout=10) as response:
                assert response.headers.get_content_type() == "application/javascript"
                hashes[path] = hashlib.sha256(response.read()).hexdigest()
        receipts.append({"site": site, "publicConfiguration": config, "assets": hashes})
    finally:
        subprocess.run(["docker", "rm", "-f", container], check=True, stdout=subprocess.DEVNULL)
assert receipts[0]["assets"] == receipts[1]["assets"], "Site settings changed compiled assets"
pathlib.Path(output).write_text(json.dumps({"passed": True, "image": image, "sites": receipts}, indent=2) + "\n")
print("Actual frontend image: two runtime OIDC sites and unchanged application assets passed")
