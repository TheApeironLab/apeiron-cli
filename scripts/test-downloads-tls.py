#!/usr/bin/env python3
"""Offline checks for the certificate renewal guardrails."""
import datetime as dt
import importlib.util
import os
import sys
from pathlib import Path
import unittest
from unittest.mock import patch
import xml.etree.ElementTree as ET

sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location("downloads_tls", Path(__file__).with_name("downloads-tls.py"))
tls = importlib.util.module_from_spec(spec)
spec.loader.exec_module(tls)


class DownloadsTLS(unittest.TestCase):
    def test_invalid_challenges_never_write_cloud_objects(self):
        for domain, token in [("other.example", "a" * 43), (tls.DOMAIN, "../install.sh"), (tls.DOMAIN, "")]:
            with self.subTest(domain=domain, token=token), patch.dict(os.environ, {
                "CERTBOT_DOMAIN": domain, "CERTBOT_TOKEN": token,
            }), patch.object(tls, "oss") as oss:
                with self.assertRaises(ValueError):
                    tls.challenge()
                oss.assert_not_called()

    def test_cleanup_only_removes_the_current_challenge(self):
        with patch.dict(os.environ, {"CERTBOT_DOMAIN": tls.DOMAIN, "CERTBOT_TOKEN": "a" * 43}), patch.object(tls, "oss") as oss:
            tls.challenge(cleanup=True)
            oss.assert_called_once_with("api", "delete-object", "--bucket", tls.BUCKET,
                                        "--key", ".well-known/acme-challenge/" + "a" * 43)

    def test_renewal_window_uses_the_oss_expiration_format(self):
        for days, expected in [(90, False), (29, True), (-1, True)]:
            expires = (dt.datetime.now(dt.timezone.utc) + dt.timedelta(days=days)).strftime("%b %d %H:%M:%S %Y GMT")
            node = ET.fromstring(f"<Cname><Certificate><ValidEndDate>{expires}</ValidEndDate></Certificate></Cname>")
            with self.subTest(days=days), patch.object(tls, "binding", return_value=node):
                self.assertEqual(tls.due(), expected)

    def test_missing_certificate_can_be_issued_but_malformed_expiry_fails(self):
        with patch.object(tls, "binding", return_value=ET.fromstring("<Cname/>")):
            self.assertTrue(tls.due())
        with patch.object(tls, "binding", return_value=ET.fromstring("<Cname><Certificate/></Cname>")):
            with self.assertRaises(ValueError):
                tls.due()

    def test_oss_errors_do_not_echo_credentials_or_private_keys(self):
        from types import SimpleNamespace
        result = SimpleNamespace(returncode=1, stderr="Error Code: AccessDenied. signed-token=secret-value", stdout="")
        with patch.object(tls.subprocess, "run", return_value=result):
            with self.assertRaises(RuntimeError) as error:
                tls.oss("api", "put-cname")
            self.assertIn("AccessDenied", str(error.exception))
            self.assertNotIn("secret-value", str(error.exception))


if __name__ == "__main__":
    unittest.main()
