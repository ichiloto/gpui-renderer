#!/usr/bin/env python3
"""Real-process IPC checks. Requires a graphical desktop; opens brief native windows."""
import json
from pathlib import Path
import subprocess
import tempfile

root = Path(__file__).resolve().parent.parent
binary = root / "target/debug/gpui-renderer"
fixture = subprocess.check_output(["python3", str(root / "scripts/fixture.py")])
shutdown = b'{"protocol":1,"type":"shutdown"}\n'


def run_case(name, data, status, kinds):
    result = subprocess.run([str(binary)], input=data, capture_output=True, timeout=10)
    events = [json.loads(line) for line in result.stdout.splitlines()]
    assert result.returncode == status, (name, result.returncode, result.stderr)
    assert all(event["protocol"] == 1 for event in events), (name, events)
    assert [event["type"] for event in events] == kinds, (name, events)
    assert not result.stdout or result.stdout.endswith(b"\n"), (name, result.stdout)
    print(f"PASS {name}: status={status}; events={','.join(kinds)}")


run_case("fixture then shutdown drains ready", fixture + shutdown, 0, ["ready"])
run_case("recoverable error drains before shutdown", fixture + b"malformed\n" + shutdown, 0, ["ready", "error"])
run_case("shutdown before hello", shutdown, 0, [])
run_case("EOF before hello is fatal", b"", 1, ["error"])
run_case("unsupported version then shutdown", b'{"protocol":2,"type":"shutdown"}\n' + shutdown, 0, ["error"])

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
