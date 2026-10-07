#!/bin/bash
# Run only in a disposable Ubuntu container. No host directories are writable.
set -euo pipefail
[ -f /.dockerenv ] && [ "$(id -u)" = 0 ] && [ ! -e /usr/local/bin/apeiron ]
: "${APEIRON_TEST_VERSION:?release version}"
apt-get update -qq
DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends sudo >/dev/null
useradd --create-home installer
mkdir /commands
cat > /commands/curl <<'EOF'
#!/bin/sh
set -eu
while [ "$#" -gt 0 ]; do
  case "$1" in
    --output) output=$2; shift 2 ;;
    https://*) url=$1; shift ;;
    *) shift ;;
  esac
done
cp "/release/${url##*/}" "$output"
EOF
chmod 755 /commands/curl

# A non-admin cannot create the command; don't report a successful installation.
if su -s /bin/sh installer -c 'PATH=/commands:/usr/local/bin:/usr/bin:/bin; export PATH; /bin/sh /checks/install.sh --version "$APEIRON_TEST_VERSION"' > /tmp/denied.log 2>&1; then
  echo 'Unexpected successful install without administrator access' >&2
  exit 1
fi
grep -q 'Administrator authorization is required' /tmp/denied.log
[ ! -e /usr/local/bin/apeiron ]

# Grant sudo inside this disposable container and test the same parent shell.
printf '%s\n' 'installer ALL=(root) NOPASSWD: /bin/sh' > /etc/sudoers.d/apeiron-test
chmod 440 /etc/sudoers.d/apeiron-test
su -s /bin/sh installer -c '
  set -eu
  PATH=/commands:/usr/local/bin:/usr/bin:/bin
  export PATH
  /bin/sh /checks/install.sh --version "$APEIRON_TEST_VERSION" && [ "$(apeiron --version)" = "$APEIRON_TEST_VERSION" ]
  apeiron --version
  /bin/sh /checks/install.sh --version "$APEIRON_TEST_VERSION" && [ "$(apeiron --version)" = "$APEIRON_TEST_VERSION" ]
  [ ! -e "$HOME/.profile" ] || ! grep -q "Apeiron CLI PATH" "$HOME/.profile"
'
[ "$(stat -c '%U:%a' /usr/local/bin/apeiron)" = root:755 ]
echo 'PASS non-admin failure, privileged install, same-shell command and repeat update'
