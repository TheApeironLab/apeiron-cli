#!/usr/bin/env python3
"""Upload large immutable release blobs with conditional OSS multipart completion."""
import argparse
from concurrent.futures import ThreadPoolExecutor
import hashlib
import json
from pathlib import Path
import re
import subprocess
import tempfile

PART_SIZE = 64 * 1024 * 1024


def api(operation, arguments):
    result = subprocess.run(['aliyun', 'ossutil', 'api', operation, *arguments,
                             '--region', 'cn-beijing', '--output-format', 'json'],
                            text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    if result.returncode:
        raise RuntimeError('OSS ' + operation + ' failed')
    return json.loads(result.stdout) if result.stdout.strip() else {}


def upload(path, key, expected, *, call=api, part_size=PART_SIZE):
    if (not re.fullmatch(r'apeiron/blobs/sha256/[a-f0-9]{64}', key)
            or key.rsplit('/', 1)[1] != expected['sha256']
            or path.is_symlink() or not path.is_file() or path.stat().st_size != expected['size']):
        raise ValueError('multipart publication requires an exact immutable blob')
    with path.open('rb') as stream:
        digest = hashlib.sha256()
        for chunk in iter(lambda: stream.read(1024 * 1024), b''):
            digest.update(chunk)
        if digest.hexdigest() != expected['sha256']:
            raise ValueError('multipart source checksum differs')
    count = (expected['size'] + part_size - 1) // part_size
    if not 1 <= count <= 10000 or not 100 * 1024 <= part_size <= 5 * 1024 ** 3:
        raise ValueError('unsupported multipart size')
    common = ['--bucket', 'apeiron-bj-cli-downloads', '--key', key]
    result = call('initiate-multipart-upload', [*common, '--forbid-overwrite', 'true',
        '--cache-control', 'public,max-age=31536000,immutable', '--metadata', 'sha256=' + expected['sha256']])
    upload_id = result['UploadId']
    if not isinstance(upload_id, str) or not upload_id:
        raise ValueError('OSS did not return an upload identity')
    completed = False
    try:
        with tempfile.TemporaryDirectory(prefix='apeiron-multipart-') as directory:
            directory = Path(directory)

            def part(number):
                temporary = directory / str(number)
                size = min(part_size, expected['size'] - (number - 1) * part_size)
                with path.open('rb') as source, temporary.open('xb') as target:
                    source.seek((number - 1) * part_size)
                    data = source.read(size)
                    if len(data) != size:
                        raise ValueError('multipart source was truncated')
                    target.write(data)
                md5 = hashlib.md5(data, usedforsecurity=False).hexdigest()
                try:
                    result = call('upload-part', [*common, '--upload-id', upload_id,
                        '--part-number', str(number), '--body', 'file://' + str(temporary)])
                    headers = {name.lower(): value for name, value in result['Header'].items()}
                    values = headers['etag']
                    if not isinstance(values, list) or len(values) != 1:
                        raise ValueError('OSS part response lacks one ETag')
                    etag = values[0]
                    if etag.strip('"').lower() != md5:
                        raise ValueError('OSS part checksum differs from the source')
                    return {'PartNumber': number, 'ETag': etag}
                finally:
                    temporary.unlink(missing_ok=True)

            with ThreadPoolExecutor(max_workers=4) as pool:
                parts = list(pool.map(part, range(1, count + 1)))
            manifest = directory / 'parts.json'
            manifest.write_text(json.dumps({'Part': parts}))
            call('complete-multipart-upload', [*common, '--upload-id', upload_id,
                '--complete-multipart-upload', 'file://' + str(manifest),
                '--forbid-overwrite', 'true', '--object-acl', 'public-read'])
            completed = True
    finally:
        if not completed:
            # Only the upload created here is aborted, never an existing object.
            call('abort-multipart-upload', [*common, '--upload-id', upload_id])


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--file', required=True, type=Path)
    parser.add_argument('--key', required=True)
    parser.add_argument('--sha256', required=True)
    parser.add_argument('--size', required=True, type=int)
    args = parser.parse_args()
    upload(args.file, args.key, {'sha256': args.sha256, 'size': args.size})
