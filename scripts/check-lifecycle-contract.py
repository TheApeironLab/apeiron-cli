#!/usr/bin/env python3
"""Cross-repository Python publisher / native CLI ABI acceptance; no cluster IO."""
import argparse
import copy
import importlib.util
import json
import subprocess
import sys
import tempfile
from pathlib import Path

sys.dont_write_bytecode = True

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--chentu-root", type=Path, required=True)
parser.add_argument("--cli", type=Path, required=True)
args = parser.parse_args()
spec = importlib.util.spec_from_file_location(
    "lifecycle_contract", args.chentu_root / "deploy/helmfile/scripts/lifecycle_contract.py"
)
contract = importlib.util.module_from_spec(spec)
spec.loader.exec_module(contract)
manifest = {"schemaVersion": 1, "kind": "chentu-release", "release": "test-2",
            "components": {"apeiron": {}, "postgres": {}}}
manifest["lifecycle"] = contract.release_contract(manifest, {})
# This is synthetic contract evidence, never a production support declaration.
manifest["lifecycle"]["components"]["apeiron"].update(
    configurationSchema="1", dataSchema="2")
manifest["lifecycle"]["backupAdapters"]["pg"] = {
    "version": "1", "kinds": ["database"], "granularity": "component",
    "status": "verified", "evidence": ["e" * 64]}
manifest["lifecycle"]["migrationRules"]["app"] = {
    "component": "apeiron", "sourceArtifacts": {"charts/app.tgz": "a" * 64},
    "sourceConfigurationSchema": "1", "sourceDataSchema": "1",
    "targetConfigurationSchema": "1", "targetDataSchema": "2", "requiredComponents": {},
    "protectionRoles": ["database"], "irreversible": True, "replaySafe": False,
    "evidence": ["e" * 64]}
manifest["lifecycle"]["upgradePaths"] = [{
    "id": "test-path", "sourceManifestSha256": "1" * 64,
    "topologies": ["single-k3s", "multi-k3s"], "architectures": ["amd64"],
    "componentSets": [["apeiron", "postgres"]], "migrationRules": ["app"],
    "evidence": ["e" * 64]}]
mutations = [
    ("valid", (), None),
    ("site-field", ("clusterUid",), "production-cluster"),
    ("schema-bool", ("schemaVersion",), True),
    ("adapter-without-evidence", ("backupAdapters", "pg", "evidence"), []),
    ("component-type", ("components", "apeiron", "class"), []),
    ("unknown-class", ("components", "apeiron", "class"), "automatic"),
    ("unknown-target-schema", ("migrationRules", "app", "targetDataSchema"), "unknown"),
    ("wrong-target-schema", ("migrationRules", "app", "targetDataSchema"), "3"),
    ("missing-source-artifacts", ("migrationRules", "app", "sourceArtifacts"), {}),
    ("source-artifact-type", ("migrationRules", "app", "sourceArtifacts"), []),
    ("unknown-protection-role", ("migrationRules", "app", "protectionRoles"), ["missing"]),
    ("unknown-rule", ("upgradePaths", 0, "migrationRules"), ["missing"]),
    ("path-without-evidence", ("upgradePaths", 0, "evidence"), []),
    ("invalid-architecture", ("upgradePaths", 0, "architectures"), ["unknown"]),
    ("unknown-component", ("upgradePaths", 0, "componentSets"), [["unknown"]]),
    ("duplicate-combination", ("upgradePaths", 0, "componentSets"), [["apeiron"], ["apeiron"]]),
]
with tempfile.TemporaryDirectory(prefix="lifecycle-contract-abi-") as directory:
    path = Path(directory) / "manifest.json"
    for name, keys, value in mutations:
        candidate = copy.deepcopy(manifest)
        if keys:
            parent = candidate["lifecycle"]
            for key in keys[:-1]:
                parent = parent[key]
            parent[keys[-1]] = value
        try:
            contract.validate_lifecycle(candidate)
            python_accepts = True
        except ValueError:
            python_accepts = False
        path.write_text(json.dumps(candidate))
        output = subprocess.run([str(args.cli.resolve()), "lifecycle", "validate-manifest",
                                 "--manifest", str(path)], capture_output=True, text=True)
        rust_accepts = output.returncode == 0
        if python_accepts != rust_accepts or python_accepts != (name == "valid"):
            raise SystemExit(f"ABI mismatch: {name}, Python={python_accepts}, Rust={rust_accepts}")
        print(f"PASS {name}: {'accepted' if rust_accepts else 'rejected'}")
print(f"PASS {len(mutations)} publisher/native CLI contract cases; no production inputs")
