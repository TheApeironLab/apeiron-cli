set -eu
export PATH=/usr/bin:/bin:/usr/sbin:/sbin
umask 077
hosts=/private/etc/hosts
[ -f "$hosts" ] && [ ! -L "$hosts" ] || exit 1
expected='APEIRON_HASH_PLACEHOLDER'
actual=$(/usr/bin/shasum -a 256 "$hosts"); actual=${actual%% *}
[ "$actual" = "$expected" ] || exit 1
work=$(/usr/bin/mktemp -d /private/etc/apeiron-access.XXXXXX)
trap '/bin/rm -f "$work/ca.crt" "$work/hosts.new"' EXIT
/bin/cp -p "$hosts" "$work/hosts.before"
/bin/cp -p "$hosts" "$work/hosts.new"
/bin/cat > "$work/hosts.new" <<'APEIRON_DELIMITER_PLACEHOLDER'
APEIRON_HOSTS_PLACEHOLDERAPEIRON_DELIMITER_PLACEHOLDER
/bin/cat > "$work/ca.crt" <<'APEIRON_DELIMITER_PLACEHOLDER'
APEIRON_CA_PLACEHOLDERAPEIRON_DELIMITER_PLACEHOLDER
/usr/bin/security add-trusted-cert -d -r trustRoot -p ssl -k /Library/Keychains/System.keychain "$work/ca.crt"
actual=$(/usr/bin/shasum -a 256 "$hosts"); actual=${actual%% *}
[ "$actual" = "$expected" ] && [ ! -L "$hosts" ] || exit 1
/bin/mv -f "$work/hosts.new" "$hosts"
/usr/bin/dscacheutil -flushcache
/usr/bin/killall -HUP mDNSResponder || true
echo "APEIRON_BACKUP=$work/hosts.before"
