import copy
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('native_acceptance', Path(__file__).parents[1] / 'scripts/native-acceptance.py')
leases = importlib.util.module_from_spec(spec)
spec.loader.exec_module(leases)


class NativeLeaseOwnership(unittest.TestCase):
    def setUp(self):
        self.settings = {'region': 'cn-beijing', 'vpcId': 'vpc-test', 'vSwitchId': 'vsw-test',
                         'securityGroupId': 'sg-test', 'imageId': 'ubuntu_22_04_x64_20G_test.vhd'}
        lease = '123-1-k3s-single-amd64'
        self.state = {'lease': lease, 'target': 'k3s-single-amd64', 'settings': self.settings,
                      'instances': ['i-test'], 'keyPair': 'apeiron-acceptance-' + lease,
                      'object': 'apeiron/acceptance-staging/' + lease + '/transport.tar.gz'}
        self.instance = {'InstanceId': 'i-test', 'InstanceType': 'ecs.u1-c1m4.2xlarge', 'InstanceChargeType': 'PostPaid',
            'ImageId': self.settings['imageId'], 'CreationTime': '2026-10-23T01:00:00Z', 'AutoReleaseTime': '2026-10-23T05:00:00Z',
            'VpcAttributes': {'VpcId': 'vpc-test', 'VSwitchId': 'vsw-test'}, 'SecurityGroupIds': {'SecurityGroupId': ['sg-test']},
            'Tags': {'Tag': [{'TagKey': 'purpose', 'TagValue': leases.PURPOSE}, {'TagKey': 'release-run', 'TagValue': lease}]}}
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.path = Path(temp.name) / 'state.json'

    def test_wrong_network_identity_tags_expiry_or_size_prevents_destructive_calls(self):
        for key, value in [('InstanceId', 'i-business'), ('Tags', {'Tag': []}), ('VpcAttributes', {'VpcId': 'vpc-business'}),
                           ('InstanceChargeType', 'PrePaid'), ('InstanceType', 'ecs.large'), ('AutoReleaseTime', '2026-10-24T01:00:00Z')]:
            with self.subTest(key=key):
                record = {**self.instance, key: value}
                self.path.write_text(json.dumps(self.state))
                calls = []
                def api(action, **args):
                    calls.append(action)
                    return {'Instances': {'Instance': [record]}}
                with patch.object(leases, 'api', api), patch.object(leases, 'run') as external:
                    with self.assertRaises(ValueError):
                        leases.cleanup(self.path)
                    self.assertEqual(calls, ['DescribeInstances'])
                    external.assert_not_called()
        with self.assertRaises(ValueError):
            leases.owned_instances(self.state, [self.instance, self.instance])

    def test_cleanup_only_deletes_this_lease_and_confirms_removal(self):
        self.path.write_text(json.dumps(self.state))
        calls = []
        def api(action, **args):
            calls.append((action, args))
            if action == 'DescribeInstances':
                return {'Instances': {'Instance': [self.instance] if len(calls) == 1 else []}}
            return {}
        with patch.object(leases, 'api', api), patch.object(leases, 'run') as external:
            leases.cleanup(self.path)
            self.assertEqual([x[0] for x in calls], ['DescribeInstances', 'DeleteInstance', 'DescribeInstances', 'DeleteKeyPairs'])
            self.assertEqual(calls[1][1]['InstanceId'], 'i-test')
            self.assertEqual(calls[-1][1]['KeyPairNames'], [self.state['keyPair']])
            self.assertIn(self.state['object'], external.call_args.args[0])

    def test_cleanup_rejects_foreign_objects_before_any_cloud_operation(self):
        for key, value in [('object', 'apeiron-cli/channels/stable.txt'), ('keyPair', 'business-key'), ('target', 'k3s-multi-amd64')]:
            self.path.write_text(json.dumps({**self.state, key: value}))
            with patch.object(leases, 'api') as api:
                with self.assertRaises(ValueError):
                    leases.cleanup(self.path)
                api.assert_not_called()

    def test_settings_cannot_expand_region_os_or_arbitrary_instance_options(self):
        leases.validate_settings(self.settings)
        for change in [{'region': 'cn-shanghai'}, {'imageId': 'ubuntu_24_04_x64_test'}, {'instanceType': 'unbounded'}]:
            with self.assertRaises(ValueError):
                leases.validate_settings({**self.settings, **change})
