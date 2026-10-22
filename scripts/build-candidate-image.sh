#!/usr/bin/env bash
# Build artifacts only. No registry push, release/tag creation or deployment.
set -euo pipefail
root=$1
out=$2
checks=$(cd "$(dirname "$0")" && pwd)
cd "$root"
test "$(git rev-parse HEAD)" = "$APEIRON_REVISION"
test -z "$(git status --porcelain)"
case "$COMPONENT" in backend|app|ops|scode) ;; *) exit 2;; esac
case "$(uname -m):$ARCHITECTURE" in x86_64:amd64|aarch64:arm64) ;; *) exit 2;; esac
python3 deploy/lib/release-version.py --tag "v$RELEASE_VERSION" --stage
mkdir -p "$out"
image="apeiron/$COMPONENT:$RELEASE_VERSION"
# Official Dockerfiles support runtime site/OIDC configuration. Leave their
# build-time fallback URLs empty for a relocatable Chentu installation.
args=(--build-arg "GIT_SHA=$APEIRON_REVISION" --build-arg "RELEASE_VERSION=$RELEASE_VERSION"
  --build-arg "BUILD_TIME=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  --build-arg FRONTEND_API_URL= --build-arg VITE_OIDC_AUTHORITY= --build-arg VITE_OIDC_CLIENT_ID=)
if [ "$COMPONENT" = scode ]; then
  # Build the agent base on each native runner; do not substitute an amd64
  # binary or depend on an unrelated registry's mutable agent-base tag.
  docker build -f deploy/images/agent-base.Dockerfile -t apeiron/candidate-agent-base .
  source deploy/images/profile.sh internet
  platform=x64; [ "$ARCHITECTURE" != arm64 ] || platform=arm64
  mkdir -p deploy/images/scode/inputs
  tmp=$(mktemp -d)
  trap 'rm -rf "$tmp"' EXIT
  asset="scode-linux-$platform.tar.gz"
  gh release download "$SCODE_VERSION" --repo sudoprivacy/sudocode --pattern "$asset" --pattern SHA256SUMS.txt --dir "$tmp"
  (cd "$tmp"; grep "  $asset\$" SHA256SUMS.txt | sha256sum -c -)
  tar -xzf "$tmp/$asset" -C "$tmp"
  install -m 0755 "$(find "$tmp" -type f -name scode -print -quit)" deploy/images/scode/inputs/scode
  deploy/images/make-receiver-deps.sh "$tmp"
  lock=$(sha256sum cli/Cargo.lock | cut -d ' ' -f 1)
  (cd "$tmp"; sha256sum -c "$lock.tar.gz.sha256")
  mkdir deploy/images/scode/inputs/receiver-deps
  tar -xzf "$tmp/$lock.tar.gz" -C deploy/images/scode/inputs/receiver-deps
  args+=(--build-arg AGENT_BASE_IMAGE=apeiron/candidate-agent-base --build-arg "RUST_IMAGE=$RUST_IMAGE")
fi
docker build "${args[@]}" -f "deploy/images/$COMPONENT.Dockerfile" -t "$image" .
case "$COMPONENT" in
  app|ops) python3 "$checks/test-candidate-frontend.py" "$image" "$out/frontend-contract.json" ;;
  backend) test "$(docker run --rm --entrypoint cat "$image" /app/VERSION)" = "$RELEASE_VERSION" ;;
  scode) docker run --rm --entrypoint /usr/local/bin/apeiron "$image" --version | grep -Fx "apeiron $RELEASE_VERSION" ;;
esac
docker image inspect "$image" > "$out/image-inspect.json"
docker save "$image" | gzip -1 > "$out/$COMPONENT-$ARCHITECTURE.tar.gz"
python3 - "$out" "$root" <<'PY'
import hashlib,json,os,pathlib,sys
out,root=map(pathlib.Path,sys.argv[1:])
image=json.loads((out/'image-inspect.json').read_text())[0]
labels=image['Config']['Labels']
assert labels['org.opencontainers.image.revision']==os.environ['APEIRON_REVISION']
assert labels['org.opencontainers.image.version']==os.environ['RELEASE_VERSION']
assert image['Architecture']==os.environ['ARCHITECTURE']
def sha(p):
 h=hashlib.sha256()
 with p.open('rb') as f:
  for b in iter(lambda:f.read(1024*1024),b''):h.update(b)
 return h.hexdigest()
archive=next(out.glob('*.tar.gz'))
recipe=root/'deploy/images'/f"{os.environ['COMPONENT']}.Dockerfile"
(out/'build-provenance.json').write_text(json.dumps({'version':os.environ['RELEASE_VERSION'],
 'revision':os.environ['APEIRON_REVISION'],'architecture':image['Architecture'],
 'component':os.environ['COMPONENT'],'imageId':image['Id'],
 'recipe':{'path':str(recipe.relative_to(root)),'sha256':sha(recipe)},
 'archive':{'file':archive.name,'sha256':sha(archive),'size':archive.stat().st_size},
 'acceptanceComplete':False},indent=2)+'\n')
PY
