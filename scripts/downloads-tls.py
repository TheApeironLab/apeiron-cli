#!/usr/bin/env python3
"""Issue and deploy the downloads domain certificate using OSS HTTP-01."""
import argparse
import datetime as dt
import json
import os
from pathlib import Path
import re
import shlex
import subprocess
import sys
import tempfile
import urllib.request
import xml.etree.ElementTree as ET

DOMAIN = "downloads.apeironlab.cn"
BUCKET = "apeiron-bj-cli-downloads"
ORIGIN = f"{BUCKET}.oss-cn-beijing.aliyuncs.com"
SERVER = "https://acme-v02.api.letsencrypt.org/directory"


def oss(*args):
    result = subprocess.run(
        ["aliyun", "ossutil", *args, "--region", "cn-beijing"],
        capture_output=True, text=True, timeout=120,
    )
    if result.returncode:
        # CLI diagnostics can contain signed URLs or the certificate payload.
        code = re.search(r"Error Code: ([A-Za-z0-9_.-]+)", result.stderr)
        raise RuntimeError(f"OSS {args[0]} failed: {code[1] if code else 'see cloud audit log'}")
    return result.stdout


def binding():
    root = ET.fromstring(oss("api", "list-cname", "--bucket", BUCKET))
    for cname in root.findall("Cname"):
        if cname.findtext("Domain") == DOMAIN and cname.findtext("Status") == "Enabled":
            return cname
    raise RuntimeError("The downloads domain must be bound to OSS first")


def challenge_token():
    if os.environ.get("CERTBOT_DOMAIN") != DOMAIN:
        raise ValueError("Unexpected challenge domain")
    token = os.environ.get("CERTBOT_TOKEN", "")
    if not re.fullmatch(r"[A-Za-z0-9_-]{20,200}", token):
        raise ValueError("Invalid challenge token")
    return token


def challenge(cleanup=False):
    key = f".well-known/acme-challenge/{challenge_token()}"
    if cleanup:
        oss("api", "delete-object", "--bucket", BUCKET, "--key", key)
        return
    validation = os.environ["CERTBOT_VALIDATION"]
    with tempfile.NamedTemporaryFile() as file:
        file.write(validation.encode())
        file.flush()
        oss("cp", file.name, f"oss://{BUCKET}/{key}", "--acl", "public-read",
            "--content-type", "text/plain", "--cache-control", "no-store", "--force", "--quiet")
    # Check the origin bytes over TLS. The CA independently checks the public
    # domain over HTTP, so a stale local DNS cache cannot hide a broken binding.
    with urllib.request.urlopen(f"https://{ORIGIN}/{key}", timeout=30) as response:
        if response.read(4096).decode() != validation:
            raise RuntimeError("Challenge did not match the uploaded bytes")


def deploy(config):
    import certifi
    from cryptography import x509
    from cryptography.hazmat.primitives import serialization

    live = config / "live" / DOMAIN
    chain = (live / "fullchain.pem").read_text()
    private = (live / "privkey.pem").read_text()
    cert = x509.load_pem_x509_certificate(chain.encode())
    key = serialization.load_pem_private_key(private.encode(), password=None)
    if cert.public_key() != key.public_key():
        raise ValueError("Certificate and key do not match")
    names = cert.extensions.get_extension_for_class(x509.SubjectAlternativeName).value
    if names.get_values_for_type(x509.DNSName) != [DOMAIN]:
        raise ValueError("Unexpected certificate names")
    if cert.not_valid_after_utc <= dt.datetime.now(dt.timezone.utc) + dt.timedelta(days=30):
        raise ValueError("New certificate expires too soon")
    subprocess.run([
        "openssl", "verify", "-CAfile", certifi.where(), "-untrusted", str(live / "chain.pem"),
        "-verify_hostname", DOMAIN, str(live / "cert.pem"),
    ], check=True, stdout=subprocess.DEVNULL)
    previous = binding().findtext("Certificate/CertId")
    root = ET.Element("BucketCnameConfiguration")
    cname = ET.SubElement(root, "Cname")
    ET.SubElement(cname, "Domain").text = DOMAIN
    certificate = ET.SubElement(cname, "CertificateConfiguration")
    ET.SubElement(certificate, "Certificate").text = chain
    ET.SubElement(certificate, "PrivateKey").text = private
    if previous:
        ET.SubElement(certificate, "PreviousCertId").text = previous
    else:
        ET.SubElement(certificate, "Force").text = "true"
    with tempfile.NamedTemporaryFile() as file:
        file.write(ET.tostring(root))
        file.flush()
        oss("api", "put-cname", "--bucket", BUCKET,
            "--cname-configuration", f"file://{file.name}")
    print(f"Deployed {DOMAIN}; expires {cert.not_valid_after_utc.isoformat()}")


def due():
    certificate = binding().find("Certificate")
    if certificate is None:
        return True
    end = certificate.findtext("ValidEndDate")
    if not end:
        raise ValueError("OSS certificate has no expiry date")
    expires = dt.datetime.strptime(end, "%b %d %H:%M:%S %Y GMT").replace(tzinfo=dt.timezone.utc)
    return expires <= dt.datetime.now(dt.timezone.utc) + dt.timedelta(days=30)


def issue(config, staging=False):
    config.mkdir(parents=True, exist_ok=True, mode=0o700)
    account = os.environ.get("DOWNLOADS_ACME_ACCOUNT")
    if account:
        data = json.loads(account)
        if not re.fullmatch(r"[a-f0-9]{32}", data["id"]):
            raise ValueError("Invalid ACME account ID")
        path = config / "accounts/acme-v02.api.letsencrypt.org/directory" / data["id"]
        path.mkdir(parents=True, exist_ok=True, mode=0o700)
        for name in ["meta.json", "private_key.json", "regr.json"]:
            target = path / name
            target.write_text(json.dumps(data[name]))
            target.chmod(0o600)
    hook = shlex.join([sys.executable, str(Path(__file__).resolve())])
    subprocess.run([
        str(Path(sys.executable).with_name("certbot")), "certonly",
        "--manual", "--preferred-challenges", "http", "--non-interactive",
        "--agree-tos", "--register-unsafely-without-email",
        "--server", SERVER.replace("acme-v02", "acme-staging-v02") if staging else SERVER,
        "--config-dir", str(config), "--work-dir", str(config / "work"),
        "--logs-dir", str(config / "logs"), "--cert-name", DOMAIN, "-d", DOMAIN,
        "--manual-auth-hook", f"{hook} auth", "--manual-cleanup-hook", f"{hook} cleanup",
    ], check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=["auth", "cleanup", "issue", "deploy", "renew", "due"])
    parser.add_argument("--config-dir", type=Path)
    parser.add_argument("--staging", action="store_true")
    parser.add_argument("--force", action="store_true", help="Force one explicit renewal for recovery or validation")
    args = parser.parse_args()
    if args.command in ("auth", "cleanup"):
        challenge(args.command == "cleanup")
    elif args.command == "due":
        print(str(due()).lower())
    else:
        if args.staging and args.command != "issue":
            parser.error("Staging certificates must never be deployed")
        if not args.config_dir or not args.config_dir.is_absolute():
            parser.error("An absolute --config-dir is required")
        if args.command == "renew" and not args.force and not due():
            print("Certificate has more than 30 days left; no renewal needed")
            return
        if args.command in ("issue", "renew"):
            issue(args.config_dir, args.staging)
        if args.command in ("deploy", "renew"):
            deploy(args.config_dir)


if __name__ == "__main__":
    main()
