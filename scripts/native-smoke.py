#!/usr/bin/env python3
"""Real-process IPC checks. Requires a graphical desktop; opens brief native windows."""
import argparse
import json
from pathlib import Path
import subprocess
import tempfile

root = Path(__file__).resolve().parent.parent
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--binary", type=Path, default=root / "target/debug/gpui-renderer")
parser.add_argument("--evidence-dir", type=Path)
args = parser.parse_args()
binary = args.binary
fixture = subprocess.check_output(["python3", str(root / "scripts/fixture.py")])
shutdown = b'{"protocol":1,"type":"shutdown"}\n'


def run_case(name, data, status, kinds, protocols=None):
    result = subprocess.run([str(binary)], input=data, capture_output=True, timeout=10)
    events = [json.loads(line) for line in result.stdout.splitlines()]
    assert result.returncode == status, (name, result.returncode, result.stderr)
    assert [event["protocol"] for event in events] == (protocols or [1] * len(events)), (name, events)
    assert [event["type"] for event in events] == kinds, (name, events)
    assert not result.stdout or result.stdout.endswith(b"\n"), (name, result.stdout)
    errors = [event["message"] for event in events if event["type"] == "error"]
    assert result.stderr.decode().splitlines() == errors, (name, result.stderr, errors)
    if args.evidence_dir:
        args.evidence_dir.mkdir(parents=True, exist_ok=True)
        stem = args.evidence_dir / name.replace(" ", "-")
        stem.with_suffix(".stdout.ndjson").write_bytes(result.stdout)
        stem.with_suffix(".stderr.log").write_bytes(result.stderr)
        stem.with_suffix(".exit").write_text(str(result.returncode) + "\n")
    print(f"PASS {name}: status={status}; events={','.join(kinds)}")


run_case("fixture then shutdown drains ready", fixture + shutdown, 0, ["ready"])
run_case("recoverable error drains before shutdown", fixture + b"malformed\n" + shutdown, 0, ["ready", "error"])
run_case("shutdown before hello", shutdown, 0, [])
run_case("EOF before hello is fatal", b"", 1, ["error"])
run_case("unsupported version then shutdown", b'{"protocol":3,"type":"shutdown"}\n' + shutdown, 0, ["error"])

# Closing the output read end must wake shutdown and preserve a nonzero status.
with tempfile.TemporaryFile() as input_file, tempfile.TemporaryFile() as stderr:
    input_file.write(fixture)
    input_file.seek(0)
    process = subprocess.Popen([str(binary)], stdin=input_file, stdout=subprocess.PIPE, stderr=stderr)
    process.stdout.close()
    assert process.wait(timeout=10) == 1, "broken stdout must be fatal"
print("PASS broken stdout: status=1")

# Do not read stdout: force backpressure without blocking the test's input writer.
with tempfile.TemporaryFile() as input_file, tempfile.TemporaryFile() as stderr:
    input_file.write(fixture + b"malformed\n" * 50000)
    input_file.seek(0)
    process = subprocess.Popen([str(binary)], stdin=input_file, stdout=subprocess.PIPE, stderr=stderr)
    assert process.wait(timeout=10) == 1, "unread stdout must time out without hanging the UI"
    process.stdout.close()
print("PASS unread stdout: bounded shutdown, status=1")

# v2 session routing and strict recovery, using real renderer processes.
v2 = subprocess.check_output(["python3", str(root / "scripts/fixture.py"), "--protocol", "2"])
shutdown2 = b'{"protocol":2,"type":"shutdown"}\n'
hello2, first2, second2 = v2.splitlines(keepends=True)
run_case("v2 fixture and style replacement shutdown", v2 + shutdown2, 0, ["ready"], [2])
run_case("v2 malformed and mixed input stays v2", v2 + b"malformed\n" + shutdown + fixture.splitlines(keepends=True)[1] + hello2 + shutdown2, 0, ["ready", "error", "error", "error", "error"], [2] * 5)
bad_hello = json.loads(hello2)
bad_hello["assetRoot"] = "/nonexistent-ichiloto-assets"
run_case("failed v2 hello then valid v2 session", json.dumps(bad_hello).encode() + b"\n" + v2 + b"malformed\n" + shutdown2, 0, ["error", "ready", "error"], [1, 2, 2])
run_case("v1 rejects mixed v2 without shutdown", fixture + shutdown2 + first2 + shutdown, 0, ["ready", "error", "error"])
bad_frame = json.loads(second2)
bad_frame["textLayers"][1]["runs"][0]["background"] = {"kind": "ansi16", "index": 16}
run_case("v2 invalid colour then valid replacement", hello2 + first2 + json.dumps(bad_frame).encode() + b"\n" + second2 + shutdown2, 0, ["ready", "error"], [2, 2])
run_case("v2 invalid run then empty replacement", hello2 + first2 + b'{"protocol":2,"type":"frame","frame":3,"textLayers":[{"id":"bad","layer":0,"runs":[{"row":99,"column":0,"text":"x","foreground":null,"background":null}]}],"sprites":[]}\n' + b'{"protocol":2,"type":"frame","frame":4,"textLayers":[],"sprites":[]}\n' + shutdown2, 0, ["ready", "error"], [2, 2])
