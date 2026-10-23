#!/usr/bin/env bash
set -euo pipefail

# The pinned setup action installs Linux binaries here, but hosted runners may
# already have a newer /usr/local/bin/aliyun earlier on PATH.
test "$(/usr/bin/aliyun version)" = '3.3.23'
directory="$RUNNER_TEMP/apeiron-release-aliyun"
mkdir -p "$directory"
ln -sf /usr/bin/aliyun "$directory/aliyun"
printf '%s\n' "$directory" >> "$GITHUB_PATH"

# aliyun lazily installs ossutil in a shared location. Complete that bootstrap
# before any concurrent immutable uploads can race over its executable.
"$directory/aliyun" ossutil version
