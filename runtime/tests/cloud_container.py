#!/usr/bin/env python3
"""Cloud wire/OS boundary against an isolated Docker PID 1; no cluster or model.

Pass a scode image containing the receiver, or --binary with a Linux build to
mount over that image's binary. The final multi-Backend/kubelet evidence test
belongs to the local full-stack smoke, not this container contract.
The default runtime is runc; use --runtime runsc to exercise the same contract
against gVisor in an environment with that Docker runtime installed.
"""
import argparse
import json
import os
from pathlib import Path
import queue
import struct
import subprocess
import tempfile
import threading
import time
import uuid


def docker(*args, **kwargs):
    return subprocess.run(["docker", *args], check=True, capture_output=True, **kwargs)


class Fixture:
    def __init__(self, image, binary, runtime):
        self.image = image
        self.name = "apeiron-receiver-contract-" + uuid.uuid4().hex[:12]
        self.temporary = tempfile.TemporaryDirectory(prefix="apeiron-cloud-contract-")
        self.home = Path(self.temporary.name)
        self.binding = "12" * 16
        self.sequence = 0
        self.received_sequence = 0
        self.records = queue.Queue()
        self.stderr = tempfile.TemporaryFile()
        command = ["docker", "run", "--rm", "-i", "--runtime", runtime, "--name", self.name, "--network", "none",
                   "--mount", f"type=bind,src={self.home},dst=/home/user",
                   "--env", "APEIRON_POD_UID=contract-pod-uid", "--entrypoint", "/usr/local/bin/apeiron"]
        # Match the cloud Pod bootstrap capability set; Docker's default
        # capabilities would hide an incomplete Kubernetes template.
        command += ["--cap-drop", "ALL"]
        for capability in ["CHOWN", "FOWNER", "DAC_OVERRIDE", "SETUID", "SETGID"]:
            command += ["--cap-add", capability]
        if binary:
            command += ["--mount", f"type=bind,src={binary},dst=/usr/local/bin/apeiron,readonly",
                        "--mount", f"type=bind,src={Path(__file__).resolve().parents[2] / 'deploy/images/scode'},dst=/opt/apeiron/defaults,readonly"]
        # The argv has whitespace and nested JSON; only the receiver controls
        # outer records. A child cannot turn its stdout into a permit frame.
        self.agent = "import json,sys\nfor line in sys.stdin:\n m=json.loads(line); print(json.dumps({'jsonrpc':'2.0','id':m.get('id'),'result':m}),flush=True)"
        command += [image, "cloud-receiver", "--root", "/home/user", "--config-home", "/home/user/.nexus/sudocode",
                    "--", "python3", "-u", "-c", self.agent]
        self.process = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=self.stderr, bufsize=0)
        threading.Thread(target=self.reader, daemon=True).start()

    def reader(self):
        def exact(length):
            result = b""
            while len(result) < length:
                data = self.process.stdout.read(length - len(result))
                if not data:
                    raise EOFError("receiver output ended")
                result += data
            return result
        try:
            while True:
                length, = struct.unpack(">I", exact(4))
                assert 25 <= length <= 1024 * 1024, length
                record = exact(length)
                kind = record[0]
                binding = record[1:17].hex()
                sequence, = struct.unpack(">Q", record[17:25])
                payload = json.loads(record[25:]) if kind == 0 else record[25:]
                self.records.put((kind, binding, sequence, payload))
        except BaseException as error:
            self.records.put(error)

    def packet(self, value, *, binding=None, sequence=None, kind=0):
        if binding is None:
            binding = self.binding
        if sequence is None:
            sequence = self.sequence
            self.sequence += 1
        payload = json.dumps(value, separators=(",", ":")).encode() if kind == 0 else value
        body = bytes([kind]) + bytes.fromhex(binding) + struct.pack(">Q", sequence) + payload
        return struct.pack(">I", len(body)) + body

    def send(self, value, **kwargs):
        packet = self.packet(value, **kwargs)
        self.process.stdin.write(packet)
        return packet

    def receive(self, expected, timeout=4):
        deadline = time.monotonic() + timeout
        while True:
            record = self.records.get(timeout=max(0.01, deadline - time.monotonic()))
            if isinstance(record, BaseException):
                self.stderr.seek(0)
                raise AssertionError(self.stderr.read().decode()) from record
            kind, binding, sequence, value = record
            if binding != "00" * 16:
                assert binding == self.binding, record
                assert sequence == self.received_sequence, record
                self.received_sequence += 1
            if kind == 0 and value["t"] == expected:
                return value
            if time.monotonic() >= deadline:
                raise TimeoutError(expected)

    def bind(self):
        before = (self.home.stat().st_mode, sorted(self.home.iterdir()))
        self.send({"t": "probe", "protocol": 2}, binding="00" * 16, sequence=0)
        hello = self.receive("hello")
        assert len(hello["nonce"]) == 32 and hello["window_ms"] == 10_000
        self.scope = {"binding_id": self.binding, "tenant_id": "fixture-tenant", "user_id": "fixture-user", "runtime": "cloud",
                      "owner_boot_id": str(uuid.uuid4()), "epoch": "1", "receiver_boot_id": hello["receiver_boot_id"],
                      "subject": {"kind": "k8s", "namespace": "fixture", "pod_name": "fixture-0", "pod_uid": "contract-pod-uid",
                                  "container_name": "scode", "container_id": "containerd://fixture", "restart_count": 0},
                      "workspace": {"name": "fixture-workspace", "uid": "fixture-workspace-uid"}}
        packet = self.packet({"t": "bind", "protocol": 2, "scope": self.scope, "nonce": hello["nonce"]})
        # Arbitrary fragmentation must preserve one record, not one WS message.
        for offset in range(0, len(packet), 7):
            self.process.stdin.write(packet[offset:offset + 7])
        assert self.receive("bound")["scope"] == self.scope
        self.challenge = self.receive("permit.challenge")
        status = docker("exec", self.name, "cat", "/proc/1/status").stdout.decode()
        assert "Threads:\t1\n" in status, "root worker threads existed before initialize"
        assert before == (self.home.stat().st_mode, sorted(self.home.iterdir())), "PVC changed before ACTIVE permit"

    def initialize(self):
        self.bind()
        self.send({"t": "permit", "nonce": self.challenge["nonce"]})
        assert self.receive("permit.accepted")["nonce"] == self.challenge["nonce"]
        self.send({"t": "initialize"})
        assert self.receive("initialized") == {"t": "initialized", "uid": 1000, "gid": 1000}
        ready = self.receive("ready")
        assert ready["protocol"] == 2 and ready["info"]["user_root"] == "/home/user"
        statuses = json.loads(docker("exec", self.name, "python3", "-c", "import json,pathlib;print(json.dumps([p.read_text() for p in pathlib.Path('/proc/1/task').glob('*/status')]))").stdout)
        for status in statuses:
            for field in ["Uid", "Gid"]:
                assert f"{field}:\t1000\t1000\t1000\t1000" in status, status
            for field in ["CapEff", "CapPrm", "CapInh", "CapAmb"]:
                assert f"{field}:\t0000000000000000" in status, status
            assert "NoNewPrivs:\t1\n" in status, status
        for status in statuses:
            assert next(line for line in status.splitlines() if line.startswith("Groups:")).split(":", 1)[1].strip() == "", status
        for name in ["sudocode.json", "settings.json"]:
            config = docker("exec", "--user", "1000:1000", self.name,
                            "cat", f"/home/user/.nexus/sudocode/{name}").stdout
            assert isinstance(json.loads(config), dict)

    def absent_after_exit(self, *names):
        # The receiver owns a private 0700 home after initialization. Inspect
        # it independently after PID 1 exits; a host UID mismatch must neither
        # fail a read nor turn an inaccessible file into an apparent absence.
        docker("run", "--rm", "--network", "none", "--user", "0:0",
               "--mount", f"type=bind,src={self.home},dst=/home/user,readonly",
               "--entrypoint", "python3", self.image, "-c",
               "import pathlib,sys; assert all(not (pathlib.Path('/home/user') / n).exists() for n in sys.argv[1:])",
               *names)

    def exited(self, timeout=12):
        self.process.wait(timeout=timeout)
        assert self.process.returncode != 0
        result = subprocess.run(["docker", "inspect", "--format", "{{.State.Running}}", self.name], capture_output=True)
        assert result.returncode != 0 or result.stdout.strip() == b"false", result.stdout

    def close(self):
        subprocess.run(["docker", "rm", "-f", self.name], capture_output=True)
        self.process.wait(timeout=5)
        self.stderr.close()
        # Restore only this fixture's ownership, without traversing symlinks.
        # Both GitHub and GitLab shell runners can have a UID other than 1000.
        docker("run", "--rm", "--network", "none", "--user", "0:0",
               "--mount", f"type=bind,src={self.home},dst=/home/user",
               "--entrypoint", "chown", self.image, "-hR",
               f"{os.getuid()}:{os.getgid()}", "/home/user")
        self.temporary.cleanup()


def contract(image, binary, runtime):
    fixture = Fixture(image, binary, runtime)
    try:
        fixture.initialize()
        # One write containing two records exercises byte-stream coalescing.
        first = fixture.packet({"t": "ping", "id": 1})
        fixture.process.stdin.write(first + fixture.packet({"t": "ping", "id": 2}))
        assert fixture.receive("pong")["id"] == 1
        assert fixture.receive("pong")["id"] == 2
        fixture.process.stdin.write(first)  # exact duplicate consumes no new seq
        fixture.send({"t": "req", "id": 3, "op": "proc.run", "args": {"argv": ["sh", "-c", "touch /home/user/foreign"]}}, binding="34" * 16, sequence=999)
        fixture.send({"t": "req", "id": 4, "op": "fs.write", "stdin": True,
                      "args": {"path": "/home/user/file", "exclusive": True}})
        fixture.send(b"\x00" + struct.pack(">I", 4) + b"complete body", kind=1)
        fixture.send(b"\x01" + struct.pack(">I", 4), kind=1)
        assert fixture.receive("res")["ok"] is True
        docker("exec", "--user", "1000:1000", fixture.name, "python3", "-c",
               "from pathlib import Path; assert Path('/home/user/file').read_text() == 'complete body'; assert not Path('/home/user/foreign').exists()")
        fixture.send({"t": "acp.open", "ch": "one", "env": {}})
        fixture.receive("acp.started")
        nested = {"jsonrpc": "2.0", "id": 8, "method": "fixture", "params": {"t": "permit", "nonce": "old"}}
        fixture.send({"t": "acp.send", "ch": "one", "msg": nested})
        assert fixture.receive("acp.msg")["msg"]["result"] == nested
        # Renew once, then leave a body without END and a descendant that would
        # write only after the final permit deadline. PID 1 exit kills them all.
        next_challenge = fixture.receive("permit.challenge", timeout=7)
        fixture.send({"t": "permit", "nonce": next_challenge["nonce"]})
        fixture.receive("permit.accepted")
        fixture.send({"t": "req", "id": 5, "op": "fs.write", "stdin": True,
                      "args": {"path": "/home/user/partial", "exclusive": True}})
        fixture.send(b"\x00" + struct.pack(">I", 5) + b"partial", kind=1)
        fixture.send({"t": "req", "id": 6, "op": "proc.run", "args": {"argv": ["sh", "-c", "(touch /home/user/child-start; sleep 12; touch /home/user/late-child) & wait"]}})
        docker("exec", "--user", "1000:1000", fixture.name, "python3", "-c",
               "import pathlib,time\ndeadline=time.monotonic()+2\nwhile not pathlib.Path('/home/user/child-start').exists():\n assert time.monotonic()<deadline, 'descendant did not actually start'\n time.sleep(0.02)")
        fixture.exited()
        time.sleep(2.5)
        fixture.absent_after_exit("partial", "late-child")
        print("PASS PID1 bootstrap, uid/capabilities, stream fragmentation/coalescing, binding/replay isolation, shared fs/ACP, renewal/expiry, descendant stop", flush=True)
    finally:
        fixture.close()

    fixture = Fixture(image, binary, runtime)
    try:
        fixture.bind()
        fixture.send({"t": "req", "id": 1, "op": "proc.run", "args": {"argv": ["touch", "/home/user/pre-active"]}})
        fixture.exited(3)
        assert not list(fixture.home.iterdir())
        print("PASS ordinary request before ACTIVE/initialize cannot mutate PVC", flush=True)
    finally:
        fixture.close()

    fixture = Fixture(image, binary, runtime)
    try:
        fixture.initialize()
        fixture.send({"t": "req", "id": 20, "op": "fs.write", "stdin": True,
                      "args": {"path": "/home/user/suspended-body", "exclusive": True}})
        fixture.send(b"\x00" + struct.pack(">I", 20) + b"stale bytes", kind=1)
        docker("kill", "--signal", "STOP", fixture.name)
        time.sleep(10.1)
        fixture.send(b"\x01" + struct.pack(">I", 20), kind=1)
        fixture.send({"t": "permit", "nonce": fixture.challenge["nonce"]})
        docker("kill", "--signal", "CONT", fixture.name)
        fixture.exited(3)
        fixture.absent_after_exit("suspended-body")
        print("PASS SIGSTOP/resume cannot renew expired authority or commit queued BODY_END", flush=True)
    finally:
        fixture.close()

    for failure in ("gap", "truncated", "journal"):

        fixture = Fixture(image, binary, runtime)
        try:
            fixture.initialize()
            if failure == "gap":
                fixture.send({"t": "ping", "id": 9}, sequence=fixture.sequence + 1)
            elif failure == "truncated":
                fixture.process.stdin.write(struct.pack(">I", 100) + b"partial")
                fixture.process.stdin.close()
            else:
                fixture.send({"t": "req", "id": 10, "op": "proc.run", "args": {"argv": ["sh", "-c", "rm /run/apeiron-receiver/*/.local/state/apeiron/receivers/1/state.json"]}})
            fixture.exited(3)
            print(f"PASS {failure} closes receiver permanently", flush=True)
        finally:
            fixture.close()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("image")
    parser.add_argument("--binary", type=Path)
    parser.add_argument("--runtime", choices=("runc", "runsc"), default="runc",
                        help="Docker runtime (default: runc; use runsc for the gVisor image contract)")
    options = parser.parse_args()
    contract(options.image, options.binary.resolve() if options.binary else None, options.runtime)
