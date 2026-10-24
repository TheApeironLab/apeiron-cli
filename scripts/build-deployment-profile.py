#!/usr/bin/env python3
"""Invoke Chentu's committed publisher with measured material and builder CLI."""
import argparse
import json
from pathlib import Path
import subprocess
import sys
from distribution_scope import required_components, validate_catalog

p = argparse.ArgumentParser(description=__doc__)
for key in ('chentu', 'candidate', 'prepared', 'builder', 'out', 'lock'):
    p.add_argument('--' + key, type=Path, required=True)
p.add_argument('--profile', choices=['k3d-arm64', 'k3s-amd64'], required=True)
a = p.parse_args()
identity = json.loads(a.candidate.read_text())
recipe = json.loads(a.lock.read_text())
catalog = json.loads((a.prepared / 'material/chentu/setup/install.json').read_text())
scope = required_components(recipe, catalog)
validate_catalog(catalog, scope)
arch = 'amd64' if a.profile == 'k3s-amd64' else 'arm64'
command = [sys.executable, str(a.chentu / 'bundle/hosted_release.py'), '--material', str(a.prepared / 'material'),
    '--out', str(a.out), '--release', identity['version'], '--profile', a.profile, '--arch', arch,
    '--delivery-scope', 'custom', '--inventory', str(a.prepared / 'inventory.yaml'),
    '--cli-binary', str(a.builder / 'apeiron'), '--cli-provenance', str(a.builder / 'build-provenance.json')]
for component in scope:
    command += ['--required-component', component]
if arch == 'amd64':
    command += ['--host-deb-proof', str(a.prepared / 'host-deb-proof.json')]
else:
    lineage = a.prepared / 'toolbox-lineage.json'
    toolbox = json.loads(lineage.read_text())
    command += ['--toolbox', str(a.prepared / 'material' / toolbox['export']['file']),
                '--toolbox-id', toolbox['derived']['imageId'], '--toolbox-provenance', str(lineage)]
subprocess.run(command, check=True)
