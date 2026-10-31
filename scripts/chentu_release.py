"""Load deployment tooling from the versioned Chentu submodule, never from PATH."""
import os
from pathlib import Path
import sys


def delegate(namespace):
    entry = Path(namespace['__file__']).name
    directory = Path(__file__).resolve().parents[1] / 'third_party/chentu/bundle/release'
    source = directory / entry
    if not source.is_file():
        raise RuntimeError('Pinned Chentu release tooling is missing; initialize third_party/chentu')
    # The coordinator owns the public delivery destination, not the deployment engine.
    os.environ['CHENTU_RELEASE_PUBLIC_BASE'] = 'https://downloads.apeironlab.cn/'
    sys.path.insert(0, str(directory))
    namespace['__file__'] = str(source)
    exec(compile(source.read_bytes(), str(source), 'exec'), namespace)
