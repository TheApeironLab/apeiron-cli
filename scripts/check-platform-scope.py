#!/usr/bin/env python3
"""Reject incomplete platform seeds before scheduling application image builds."""
import hashlib
import json
from pathlib import Path
import urllib.request

from distribution_scope import required_components, validate_catalog

root = Path(__file__).parents[1]
recipe = json.loads((root / 'release/recipe.json').read_text())
scope = required_components(recipe)
for profile, record in recipe['platforms'].items():
    url = 'https://downloads.apeironlab.cn/apeiron/blobs/sha256/' + record['sha256']
    if record['url'] != url or record['size'] > 8 * 1024 * 1024:
        raise ValueError('invalid platform seed identity')
    with urllib.request.urlopen(url, timeout=60) as response:
        if response.url != url:
            raise ValueError('platform seed redirects are not accepted')
        data = response.read(record['size'] + 1)
    if len(data) != record['size'] or hashlib.sha256(data).hexdigest() != record['sha256']:
        raise ValueError('platform seed checksum mismatch')
    seed = json.loads(data)
    if seed['profile'] != profile or seed['kind'] != 'apeiron-platform-material':
        raise ValueError('platform seed profile mismatch')
    validate_catalog(seed['catalog'], scope)
    missing = {item['path'] for item in seed['catalog']['files']} - set(seed['assets']) - set(seed['rebuiltResources'])
    if missing:
        raise ValueError('platform seed lacks resource identities: ' + ', '.join(sorted(missing)))
    print('PASS complete wizard scope: ' + profile)
