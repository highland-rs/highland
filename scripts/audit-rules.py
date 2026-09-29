#!/usr/bin/env python3
"""
Differential audit: does the shipped binary reject what the documentation says it
rejects?

Each case below is a configuration that violates exactly one rule from
docs/user/configuration.md's table. The script runs `highland check-config` on
each and reports the rules that passed validation.

A rule listed here as UNENFORCED is a rule the documentation promises and the
product does not apply. V-08 was found this way -- by asking the installed
binary to reject a loopback peer and being told the file was valid.
"""
import subprocess
import sys
import tempfile
import os

BIN = sys.argv[1] if len(sys.argv) > 1 else "target/debug/highland"

BASE = """schema_version = 1

[node]
name = "audit"

[[instance]]
name = "api"
interface = "lo"
vrid = 42
priority = 150

[instance.network]
mode = "unicast"
peers = ["192.0.2.20"]

[[instance.vip]]
address = "192.0.2.10/24"
"""

def case(rule, note, body):
    return (rule, note, body)

CASES = [
    case("V-01", "vrid 0", BASE.replace("vrid = 42", "vrid = 0")),
    case("V-02", "priority 0", BASE.replace("priority = 150", "priority = 0")),
    case("V-03", "mixed families in one instance",
         BASE.replace('address = "192.0.2.10/24"',
                      'address = "192.0.2.10/24"\n\n[[instance.vip]]\naddress = "2001:db8::10/64"')),
    case("V-04", "interval above 40.95s",
         BASE.replace('priority = 150', 'priority = 150\nadvertisement_interval = "60s"')),
    case("V-05", "duplicate instance name", BASE + "\n" + BASE.split("[[instance]]")[1].join(["[[instance]]", ""]).join(["", ""])),
    case("V-08", "a peer configured on this node",
         BASE.replace('peers = ["192.0.2.20"]', 'peers = ["127.0.0.1"]')),
    case("V-22", "an interface that does not exist",
         BASE.replace('interface = "lo"', 'interface = "nosuchif0"')),
    case("V-07", "v4 VIP with no v4 peer",
         BASE.replace('mode = "unicast"\npeers = ["192.0.2.20"]', 'mode = "unicast"\npeers = []')),
    case("V-09", "multicast address as a unicast peer",
         BASE.replace('peers = ["192.0.2.20"]', 'peers = ["224.0.0.18"]')),
    case("V-11", "an instance with no VIP",
         BASE.replace('[[instance.vip]]\naddress = "192.0.2.10/24"\n', "")),
    case("V-13", "prefix 0", BASE.replace('192.0.2.10/24', '192.0.2.10/0')),
    case("V-28", "unsupported schema_version", BASE.replace("schema_version = 1", "schema_version = 2")),
    case("V-31", "no instances at all",
         'schema_version = 1\n\n[node]\nname = "audit"\n'),
    case("V-32", "blank node name", BASE.replace('name = "audit"', 'name = ""')),
    case("V-24", "multicast TTL not 255",
         BASE.replace('mode = "unicast"\npeers = ["192.0.2.20"]',
                      'mode = "multicast"\n\n[instance.network.multicast]\nttl = 1')),
    case("V-30", "metrics.enabled and metrics.listen disagreeing",
         BASE + '\n[metrics]\nenabled = true\n'),
]

# V-05 is easier to write out than to derive.
CASES[4] = ("V-05", "duplicate instance name",
            BASE + '\n[[instance]]\nname = "api"\ninterface = "lo"\nvrid = 43\npriority = 150\n'
            '[instance.network]\nmode = "unicast"\npeers = ["192.0.2.20"]\n'
            '[[instance.vip]]\naddress = "192.0.2.11/24"\n')

unenforced, malformed = [], []
for rule, note, body in CASES:
    with tempfile.NamedTemporaryFile("w", suffix=".toml", delete=False) as fh:
        fh.write(body)
        path = fh.name
    try:
        os.chmod(path, 0o644)
        proc = subprocess.run([BIN, "check-config", path], capture_output=True, text=True, timeout=30)
        combined = proc.stdout + proc.stderr
        if proc.returncode == 0:
            unenforced.append((rule, note, combined.strip().splitlines()[0] if combined.strip() else ""))
        elif "could not" in combined.lower() and "is valid" not in combined:
            # A parse/IO failure rather than a semantic rejection: the case itself
            # is wrong, which is a finding about this script, not the product.
            malformed.append((rule, note, combined.strip()[:160]))
    finally:
        os.unlink(path)

print(f"binary: {BIN}")
print(f"cases:  {len(CASES)}\n")
if unenforced:
    print("UNENFORCED (documented as rejected, accepted in fact):")
    for rule, note, out in unenforced:
        print(f"  {rule}  {note}")
        if out:
            print(f"        -> {out}")
else:
    print("UNENFORCED: none")
if malformed:
    print("\nNOT A SEMANTIC REJECTION (case may be malformed):")
    for rule, note, out in malformed:
        print(f"  {rule}  {note}\n        -> {out}")
sys.exit(1 if (unenforced or malformed) else 0)
