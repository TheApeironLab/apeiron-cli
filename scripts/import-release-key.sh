#!/usr/bin/env bash
set -euo pipefail
# Only protected preparation jobs receive this key. Never echo its bytes.
: "${GNUPGHOME:?}" "${CHENTU_SIGNING_KEY:?}" "${CHENTU_SIGNING_FINGERPRINT:?}"
mkdir -m 700 "$GNUPGHOME"
printf '%s' "$CHENTU_SIGNING_KEY" | gpg --batch --import >/dev/null 2>&1
actual=$(gpg --batch --with-colons --list-secret-keys | awk -F: '$1=="fpr" {print $10; exit}')
test "$actual" = "$CHENTU_SIGNING_FINGERPRINT"
gpg --batch --export "$actual" > "$GNUPGHOME/public.gpg"
gpg --show-keys --with-colons release/signing-key.asc | awk -F: '$1=="fpr" {print $10; exit}' | grep -Fx "$actual" >/dev/null
