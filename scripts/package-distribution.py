#!/usr/bin/env python3
"""Combine validated Chentu profile outputs; never patch deployment source."""
import argparse
from concurrent.futures import ThreadPoolExecutor
import copy
from datetime import date
import gzip
import hashlib
import io
import importlib.util
import json
from pathlib import Path, PurePosixPath
import re
import tarfile
import urllib.request

PUBLIC = "https://downloads.apeironlab.cn/"
PROFILES = {"k3d-arm64": "arm64", "k3s-amd64": "amd64", "k3s-arm64": "arm64"}
APEIRON_IMAGES = ("backend", "app", "ops", "scode")
scope_spec = importlib.util.spec_from_file_location('distribution_scope', Path(__file__).with_name('distribution_scope.py'))
scope = importlib.util.module_from_spec(scope_spec)
scope_spec.loader.exec_module(scope)


def identity(path):
    if path.is_symlink() or not path.is_file():
        raise ValueError("release input is not an ordinary file")
    with path.open("rb") as stream:
        hasher = hashlib.sha256()
        for chunk in iter(lambda: stream.read(1024 * 1024), b''):
            hasher.update(chunk)
        digest = hasher.hexdigest()
    return {"sha256": digest, "size": path.stat().st_size}


def safe_file(root, name):
    if not isinstance(name, str) or not name or "\\" in name:
        raise ValueError("invalid release path")
    if name.startswith("/") or any(p in ("", ".", "..", ".git") for p in name.split("/")):
        raise ValueError("invalid release path")
    path = root
    for part in PurePosixPath(name).parts:
        path = path / part
        if path.is_symlink():
            raise ValueError("release path traverses a symlink")
    return path


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        raise ValueError('public profile resource redirects are not accepted')


def public_resources(directory):
    marker = directory / 'profile-handoff.json'
    if not marker.exists():
        return {}
    if marker.is_symlink():
        raise ValueError('profile handoff cannot be a symlink')
    value = json.loads(marker.read_text())
    if value.get('schemaVersion') != 1 or value.get('kind') != 'apeiron-published-profile':
        raise ValueError('invalid profile handoff')
    records = value['resources']
    if not isinstance(records, dict):
        raise ValueError('invalid public resource map')
    for name, record in records.items():
        if safe_file(directory, name).exists():
            raise ValueError('public profile resource also exists locally')
        if (set(record) != {'sha256', 'size', 'url'}
                or not re.fullmatch('[a-f0-9]{64}', record['sha256'])
                or type(record['size']) is not int or record['size'] < 0
                or record['url'] != PUBLIC + 'apeiron/blobs/sha256/' + record['sha256']):
            raise ValueError('public profile resource must have an exact content address')
    return records


def executables(directory):
    """Executable files of a profile; a CI handoff lists them because artifacts drop modes."""
    marker = directory / 'profile-handoff.json'
    if not marker.exists():
        return None
    names = json.loads(marker.read_text())['executables']
    if not isinstance(names, list) or not all(isinstance(name, str) for name in names):
        raise ValueError('invalid profile executable list')
    return set(names)


def verify_public_resource(record):
    # Blobs are immutable and content addressed. The uploader read each new
    # object back in full and recorded its sha256, so headers prove identity.
    request = urllib.request.Request(record['url'], method='HEAD')
    with urllib.request.build_opener(NoRedirect).open(request, timeout=60) as response:
        if response.status != 200:
            raise ValueError('public profile resource is unavailable')
        got = {'sha256': response.headers.get('x-oss-meta-sha256'),
               'size': int(response.headers.get('Content-Length', -1))}
    if got != {k: record[k] for k in ('sha256', 'size')}:
        raise ValueError('public profile bytes differ from the catalog')
    return got


def resource_files(catalog):
    """Complete publication resources; imageObjects are lazy installer resources."""
    objects = catalog.get('imageObjects', [])
    if not isinstance(objects, list):
        raise ValueError('invalid image object resources')
    rows = [*catalog['files'], *objects]
    if len({r['path'] for r in rows}) != len(rows):
        raise ValueError('duplicate installation/object resource path')
    return rows


def read_profile(directory, verified_public=None):
    verified_public = verified_public if verified_public is not None else {}
    records = public_resources(directory)
    catalog = json.loads((directory / 'chentu/setup/install.json').read_text())
    catalog_files = {entry['path']: entry for entry in resource_files(catalog)}
    for name, record in records.items():
        if name not in catalog_files or any(record[k] != catalog_files[name][k] for k in ('sha256', 'size')):
            raise ValueError('handoff resource differs from the catalog')
    sums = {}
    for line in (directory / "SHA256SUMS").read_text().splitlines():
        match = re.fullmatch(r"([a-f0-9]{64})  (.+)", line)
        if not match or match[2] in sums:
            raise ValueError("invalid or duplicate profile checksum")
        sums[match[2]] = match[1]
        got = records.get(match[2]) or identity(safe_file(directory, match[2]))
        if got['sha256'] != match[1]:
            raise ValueError("profile bytes differ from checksum")
    actual = {p.relative_to(directory).as_posix() for p in directory.rglob("*")
              if p.is_file() and p.relative_to(directory).as_posix() not in ('SHA256SUMS', 'profile-handoff.json')}
    actual |= set(records)
    if set(sums) != actual:
        raise ValueError("profile checksum inventory is incomplete")
    pending = {(r['sha256'], r['size']): r for r in records.values()
               if (r['sha256'], r['size']) not in verified_public}
    with ThreadPoolExecutor(max_workers=4) as pool:
        for key, got in zip(pending, pool.map(verify_public_resource, pending.values())):
            verified_public[key] = got
    return (json.loads((directory / "setup/release-manifest.json").read_text()),
            catalog,
            json.loads((directory / "chentu/chentu-package.json").read_text()))


def combine(outputs, candidate, out):
    if set(outputs) != set(PROFILES):
        raise ValueError("every release profile is required: " + ", ".join(sorted(PROFILES)))
    version = candidate["version"]
    if not re.fullmatch(r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-nightly\.[0-9]{8}\.[1-9][0-9]*)?", version):
        raise ValueError("invalid distribution version")
    if '-nightly.' in version:
        stamp = version.split('-nightly.')[1].split('.')[0]
        date(int(stamp[:4]), int(stamp[4:6]), int(stamp[6:8]))
    for name in ('chentu', 'apeiron', 'coordinator', 'systemSkills'):
        if not re.fullmatch('[a-f0-9]{40}', candidate['sources'].get(name, '')):
            raise ValueError('candidate requires full source identities')
    catalogs, packages, blobs, files, sources = {}, {}, {}, {}, None
    manifests = {}
    verified_public = {}
    images = {name: {} for name in APEIRON_IMAGES}
    for profile, directory in outputs.items():
        manifest, catalog, package = read_profile(directory, verified_public)
        remote = public_resources(directory)
        expected = candidate["sources"]
        if (manifest["release"] != version or set(manifest["profiles"]) != {profile}
                or manifest["profiles"][profile]["architecture"] != PROFILES[profile]
                or manifest["sources"]["chentu"]["commit"] != expected["chentu"]
                or manifest["sources"]["apeiron"]["commit"] != expected["apeiron"]
                or manifest["sources"]["apeiron"]["version"] != version
                or manifest["sources"]["apeironCli"]["commit"] != expected["coordinator"]
                or manifest["sources"]["systemSkills"]["commit"] != expected["systemSkills"]
                or package["revision"] != expected["chentu"] or package["version"] != version
                or package["sourceOverlay"] != {}):
            raise ValueError("profile does not bind the candidate source and version")
        if sources is not None and sources != manifest["sources"]:
            raise ValueError("profile source provenance differs")
        sources = manifest["sources"]
        if catalog["schemaVersion"] != 1 or set(catalog["targets"]) != {profile}:
            raise ValueError("incorrect profile catalog")
        scope.validate_catalog(catalog)
        catalog = copy.deepcopy(catalog)
        listed = set()
        for entry in resource_files(catalog):
            path = safe_file(directory, entry["path"])
            got = (verified_public[(entry['sha256'], entry['size'])]
                   if entry['path'] in remote else identity(path))
            if entry["path"] in listed or got != {k: entry[k] for k in ("sha256", "size")}:
                raise ValueError("catalog file identity differs")
            listed.add(entry["path"])
            key = "apeiron/blobs/sha256/" + got["sha256"]
            if entry['path'] not in remote:
                blobs.setdefault(key, {"path": str(path.resolve()), **got})
            entry["url"] = PUBLIC + key
        for component, detail in catalog["components"].items():
            if not set(detail["files"]) <= listed:
                raise ValueError("component references missing material")
        # Image identities live only in install.json (release manifest v2 lists no images).
        for item in {(i["reference"], i["digest"]) for c in catalog["components"].values()
                     for i in c.get("images", [])}:
            for component in images:
                if item[0] == f"apeiron/{component}:{version}":
                    if not re.fullmatch(r"sha256:[a-f0-9]{64}", item[1]):
                        raise ValueError("invalid platform manifest digest")
                    pinned = f"apeiron/{component}@{item[1]}"
                    # Profiles of one architecture must carry the same application image.
                    if images[component].setdefault(PROFILES[profile], pinned) != pinned:
                        raise ValueError("profiles of one architecture differ in application image")
        runtime = {}
        listed = executables(directory)
        for name, digest in package["files"].items():
            path = safe_file(directory / "chentu", name)
            if identity(path)["sha256"] != digest:
                raise ValueError("runtime source differs from package provenance")
            executable = (path.stat().st_mode & 0o111 if listed is None else f"chentu/{name}" in listed)
            runtime[name] = (path.read_bytes(), 0o755 if executable else 0o644)
        if files and runtime != files:
            raise ValueError("profiles contain different deployment runtime bytes")
        files = runtime
        catalogs[profile] = catalog
        packages[profile] = package
        manifests[profile] = manifest
    if any(set(value) != {"amd64", "arm64"} for value in images.values()):
        raise ValueError("four application images on both architectures are required")
    catalog_bytes = (json.dumps({"schemaVersion": 2, "catalogs": catalogs}, indent=2) + "\n").encode()
    first = next(iter(packages.values()))
    common = ('schemaVersion', 'kind', 'version', 'revision', 'sourceTree', 'sourceNormalization',
              'runtimePayload', 'entrypoints', 'sourceOverlay', 'files')
    if any({k: value[k] for k in common} != {k: first[k] for k in common} for value in packages.values()):
        raise ValueError('profile source package provenance differs')
    package = {key: first[key] for key in ("schemaVersion", "kind", "version", "revision", "sourceTree",
                                          "sourceNormalization", "runtimePayload", "entrypoints", "sourceOverlay", "files")}
    package["profiles"] = {name: {key: value.get(key) for key in ("deliveryScope", "thirdPartySources", "hostDebClosure", "chentuWheel")}
                           for name, value in packages.items()}
    package["generatedFiles"] = {"setup/install.json": hashlib.sha256(catalog_bytes).hexdigest()}
    files["setup/install.json"] = (catalog_bytes, 0o644)
    files["chentu-package.json"] = ((json.dumps(package, indent=2) + "\n").encode(), 0o644)
    out.mkdir(parents=True, exist_ok=False)
    archive = out / f"chentu-{version}.tar.gz"
    with archive.open("xb") as raw, gzip.GzipFile(fileobj=raw, mode="wb", filename="", mtime=0) as compressed:
        with tarfile.open(fileobj=compressed, mode="w", format=tarfile.PAX_FORMAT) as target:
            for name, (data, mode) in sorted(files.items()):
                member = tarfile.TarInfo(f"chentu-{version}/{name}")
                member.mode, member.size, member.mtime = mode, len(data), 0
                target.addfile(member, io.BytesIO(data))
    record = identity(archive)
    key = f"chentu/releases/{version}/{archive.name}"
    blobs[key] = {"path": str(archive.resolve()), **record}
    provenance = out / 'provenance.json'
    provenance.write_text(json.dumps({'schemaVersion': 1, 'candidate': candidate,
        'archive': {'file': archive.name, **record}, 'profiles': manifests}, indent=2) + '\n')
    blobs[f'chentu/releases/{version}/provenance.json'] = {'path': str(provenance.resolve()), **identity(provenance)}
    # Acceptance must cover the package's applications; every profile shares one declaration.
    applications = {json.dumps(scope.application_releases(c), sort_keys=True) for c in catalogs.values()}
    if len(applications) != 1:
        raise ValueError("profiles declare different applications")
    distribution = {"schemaVersion": 1, "version": version,
                    "applications": json.loads(applications.pop()),
                    "apeiron": {"version": version, "revision": candidate["sources"]["apeiron"], "images": images},
                    "chentu": {"version": version, "revision": candidate["sources"]["chentu"],
                               "sha256": record["sha256"], "url": PUBLIC + key}}
    (out / "distribution-input.json").write_text(json.dumps(distribution, indent=2) + "\n")
    (out / "upload-plan.json").write_text(json.dumps({"version": version, "objects": blobs}, indent=2) + "\n")
    return distribution


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--candidate", type=Path, required=True)
    parser.add_argument("--profile", action="append", required=True, metavar="PROFILE=DIRECTORY")
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    profiles = [item.split("=", 1) for item in args.profile]
    if len({item[0] for item in profiles}) != len(profiles):
        parser.error("duplicate profile")
    result = combine({name: Path(path).resolve() for name, path in profiles}, json.loads(args.candidate.read_text()), args.out)
    print(json.dumps(result))
