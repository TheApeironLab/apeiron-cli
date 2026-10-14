"""Read every archived regular file without extraction and verify exact membership."""
import hashlib
import json
import pathlib
import sys
import tarfile

archive, candidate = sys.argv[1:]
seen = {}
checksum_text = None
with tarfile.open(archive, "r:") as stream:
    for member in stream:
        path = pathlib.PurePosixPath(member.name)
        if path.is_absolute() or ".." in path.parts or path.parts[0] != candidate:
            raise ValueError("unsafe archive path")
        if member.isdir():
            continue
        if not member.isfile() or member.name in seen:
            raise ValueError("non-regular or duplicate archive member")
        digest = hashlib.sha256()
        count = 0
        content = bytearray()
        is_checksums = member.name == candidate + "/SHA256SUMS"
        with stream.extractfile(member) as source:
            while chunk := source.read(1024 * 1024):
                digest.update(chunk)
                count += len(chunk)
                if is_checksums:
                    content.extend(chunk)
        if count != member.size:
            raise ValueError("truncated member")
        seen[member.name] = digest.hexdigest()
        if is_checksums:
            checksum_text = content.decode()
if checksum_text is None:
    raise ValueError("missing checksum manifest")
expected = {}
for line in checksum_text.splitlines():
    digest, path = line.split("  ", 1)
    name = candidate + "/" + path
    if name in expected:
        raise ValueError("duplicate checksum")
    expected[name] = digest
actual = {k: v for k, v in seen.items() if k != candidate + "/SHA256SUMS"}
if actual != expected:
    raise ValueError("archive contents do not exactly match internal checksums")
print(json.dumps({"status": "passed", "regularFiles": len(seen), "checksummedFiles": len(expected)}))
