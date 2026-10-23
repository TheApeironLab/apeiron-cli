import hashlib
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('multipart_upload', Path(__file__).parents[1] / 'scripts/multipart-deployment-upload.py')
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class ImmutableMultipart(unittest.TestCase):
    def run_upload(self, *, conflict=False, corrupt_part=False):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'image.tar'
            data = b'first-part' * 11000 + b'last-part' * 1000
            path.write_bytes(data)
            expected = {'sha256': hashlib.sha256(data).hexdigest(), 'size': len(data)}
            calls, parts = [], {}

            def call(operation, arguments):
                calls.append((operation, arguments))
                self.assertEqual(arguments[arguments.index('--key') + 1], 'apeiron/blobs/sha256/' + expected['sha256'])
                if operation == 'initiate-multipart-upload':
                    self.assertEqual(arguments[arguments.index('--forbid-overwrite') + 1], 'true')
                    return {'UploadId': 'owned-upload'}
                self.assertEqual(arguments[arguments.index('--upload-id') + 1], 'owned-upload')
                if operation == 'upload-part':
                    number = int(arguments[arguments.index('--part-number') + 1])
                    body = Path(arguments[arguments.index('--body') + 1].removeprefix('file://')).read_bytes()
                    parts[number] = body
                    return {'Header': {'Etag': ['bad-checksum' if corrupt_part else '"' + hashlib.md5(body, usedforsecurity=False).hexdigest() + '"']}}
                if operation == 'complete-multipart-upload':
                    self.assertEqual(arguments[arguments.index('--forbid-overwrite') + 1], 'true')
                    manifest = json.loads(Path(arguments[arguments.index('--complete-multipart-upload') + 1].removeprefix('file://')).read_text())
                    self.assertEqual([p['PartNumber'] for p in manifest['Part']], [1, 2])
                    self.assertEqual(b''.join(parts[n] for n in sorted(parts)), data)
                    if conflict:
                        raise RuntimeError('server rejected existing immutable object')
                return {}

            if conflict or corrupt_part:
                with self.assertRaises((RuntimeError, ValueError)):
                    module.upload(path, 'apeiron/blobs/sha256/' + expected['sha256'], expected, call=call, part_size=100 * 1024)
                self.assertEqual(calls[-1][0], 'abort-multipart-upload')
                if corrupt_part:
                    self.assertNotIn('complete-multipart-upload', [c[0] for c in calls])
            else:
                module.upload(path, 'apeiron/blobs/sha256/' + expected['sha256'], expected, call=call, part_size=100 * 1024)
                self.assertEqual(calls[-1][0], 'complete-multipart-upload')

    def test_exact_parts_are_conditionally_completed(self):
        self.run_upload()

    def test_conflicting_completion_aborts_only_the_owned_upload(self):
        self.run_upload(conflict=True)

    def test_bad_part_checksum_cannot_be_completed(self):
        self.run_upload(corrupt_part=True)


if __name__ == '__main__':
    unittest.main()
