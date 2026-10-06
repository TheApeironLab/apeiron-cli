#!/bin/bash
# Native or QEMU smoke test inside a fresh Ubuntu image without Node.js/Bun.
set -euo pipefail
version=$(/candidate/apeiron --version)
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+ ]]
/candidate/apeiron init --no-open --config /tmp/apeiron-smoke/config.json > /tmp/setup.log 2>&1 &
pid=$!
trap 'kill "$pid" 2>/dev/null || true; wait "$pid" 2>/dev/null || true' EXIT
url=
for ((i=0; i<60; i++)); do
  url=$(sed -n 's/^url[[:space:]]*//p' /tmp/setup.log)
  [ -z "$url" ] || break
  kill -0 "$pid" || { cat /tmp/setup.log; exit 1; }
  sleep 1
done
[[ "$url" =~ ^http://127\.0\.0\.1:([0-9]+)(/setup/[a-f0-9]+/)$ ]]
port=${BASH_REMATCH[1]}
path=${BASH_REMATCH[2]}
request() {
  exec 3<>"/dev/tcp/127.0.0.1/$port"
  printf 'GET %s HTTP/1.1\r\nHost: 127.0.0.1:%s\r\nConnection: close\r\n\r\n' "$1" "$port" >&3
  cat <&3 > "$2"
  exec 3>&-
  head -n 1 "$2" | grep -q '200 OK'
}
request "$path" /tmp/page
grep -q 'href="./favicon.ico"' /tmp/page
request "${path}favicon.ico" /tmp/favicon
request "${path}api/config" /tmp/config
grep -q '"phase":"idle"' /tmp/config
echo "PASS Linux $(uname -m): standalone $version, setup, favicon, configuration API"
