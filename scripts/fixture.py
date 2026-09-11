#!/usr/bin/env python3
"""Emit a portable, deterministic v1 or v2 fixture (no renderer dependency)."""
import argparse
import json
from pathlib import Path

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--protocol", type=int, choices=[1, 2], default=1)
parser.add_argument("--frame", choices=["first", "second", "all"], default="all")
args = parser.parse_args()
fixtures = Path(__file__).resolve().parent.parent / "fixtures"
name = "home-frame.ndjson" if args.protocol == 1 else "presentation-v2.ndjson"
for line in (fixtures / name).read_text().splitlines():
    message = json.loads(line)
    if message["type"] == "hello":
        message["assetRoot"] = str(fixtures)
    elif args.frame != "all" and message["frame"] != (1 if args.frame == "first" else 2):
        continue
    print(json.dumps(message, separators=(",", ":")), flush=True)
