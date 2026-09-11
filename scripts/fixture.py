#!/usr/bin/env python3
"""Emit the committed fixture with this checkout's absolute asset root."""
import json
from pathlib import Path

fixtures = Path(__file__).resolve().parent.parent / "fixtures"
for line in (fixtures / "home-frame.ndjson").read_text().splitlines():
    message = json.loads(line)
    if message["type"] == "hello":
        message["assetRoot"] = str(fixtures)
    print(json.dumps(message, separators=(",", ":")), flush=True)
