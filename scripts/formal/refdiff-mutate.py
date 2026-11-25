#!/usr/bin/env -S python3 -I
"""A `vimanam` stand-in that corrupts the real binary's v1 records, for the
harness's negative checks (scripts/formal/check-refdiff-negative.sh).

Runs `$REFDIFF_REAL_BIN` with the given arguments, then, by env `OP`:
  drop    removes Review records at a `$ref` key whose pointer contains `PTR`;
  drop-any removes every record, whatever its severity, whose pointer
          contains `PTR`;
  rename  sets the values of those records to `#/components/schemas/$TO`;
  inject  adds a Review record `removed #/components/schemas/$TO` at `PTR`,
          copied from the first schema change (same endpoint and use; the
          same owner, or component `$OWNER` when set);
  flip    rates Breaking records whose pointer contains `PTR` Non-breaking;
  rate    rates every record whose pointer contains `PTR` as `$SEV`.
"""
import copy
import json
import os
import subprocess
import sys

PREFIX = "#/components/schemas/"


def main() -> int:
    proc = subprocess.run([os.environ["REFDIFF_REAL_BIN"], *sys.argv[1:]], capture_output=True, text=True)
    if proc.returncode not in (0, 3):
        sys.stderr.write(proc.stderr)
        return proc.returncode
    doc = json.loads(proc.stdout)
    op, ptr, to = os.environ["OP"], os.environ.get("PTR", ""), os.environ.get("TO", "")
    changes = doc.get("changes", [])

    def sc(c):
        return c.get("details", {}).get("schema_change")

    out = []
    for c in changes:
        s = sc(c)
        at_ref = s is not None and c["severity"] == "review" and s["pointer"].endswith("/$ref") and ptr in s["pointer"]
        if op == "drop" and at_ref:
            continue
        if op == "drop-any" and s is not None and ptr in s["pointer"]:
            continue
        if op == "rename" and at_ref:
            for side in ("before", "after"):
                if s[side]["present"] and isinstance(s[side].get("value"), str):
                    s[side]["value"] = PREFIX + to
        if op == "flip" and s is not None and ptr in s["pointer"] and c["severity"] == "breaking":
            c["severity"] = "non_breaking"
        if op == "rate" and s is not None and ptr in s["pointer"]:
            c["severity"] = os.environ["SEV"]
        out.append(c)
    if op == "inject":
        c = copy.deepcopy(next(c for c in changes if sc(c)))
        s = sc(c)
        s["pointer"], s["operation"] = ptr, "removed"
        s["before"], s["after"] = {"present": True, "value": PREFIX + to}, {"present": False}
        c["severity"] = "review"
        owner = os.environ.get("OWNER")
        if owner:
            c["details"]["component"] = owner
            c["details"].pop("old_component", None)
        out.append(c)
    doc["changes"] = out
    print(json.dumps(doc))
    return proc.returncode


if __name__ == "__main__":
    sys.exit(main())
