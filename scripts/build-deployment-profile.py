#!/usr/bin/env python3
"""Compatibility entry; implementation belongs to the pinned Chentu source."""
from pathlib import Path
import runpy

runpy.run_path(str(Path(__file__).with_name("chentu_release.py")))["delegate"](globals())
