"""Join the native inventory observations to every cooked weapon class.

Construction/retirement evidence is deliberately separate from combat parity.
Usage: python scripts/analyze_weapon_inventory.py <combat_manual run directory>
"""
import argparse
import collections
import json
from pathlib import Path
import re


def analyze(run: Path, manifest: Path):
    observations = collections.defaultdict(list)
    for file in sorted(run.glob("inst*/.parity_results.txt")):
        current = None
        identities = {}
        for line in file.read_text(encoding="utf-8-sig", errors="replace").splitlines():
            match = re.search(r"INVENTORY class=(\S+) path=(\S+) cdo=", line)
            if match:
                identifier, path = match.groups()
                native_class = path.rsplit(".", 1)[-1]
                current = {"id": identifier, "class": native_class, "instance": file.parent.name,
                           "path": path, "geometry_observations": [], "cleanup_observations": []}
                identities[identifier] = current
                observations[native_class].append(current)
            elif current and "COLLIDERS " in line:
                current["geometry_observations"].append(line)
            elif "INVENTORY cleanup_probe " in line:
                match = re.search(r"class=(\S+) .*status=(\S+)", line)
                if match and match[1] in identities:
                    identities[match[1]]["cleanup_observations"].append(line)
            elif "INVENTORY result " in line:
                match = re.search(r"class=(\S+) inspected=(true|false) cleanup=(true|false) reason=(.*)", line)
                if match and match[1] in identities:
                    row = identities[match[1]]
                    row.update(construction_inspected=match[2] == "true", retired=match[3] == "true", reason=match[4])
                    current = None
    rows = []
    for native in json.loads(manifest.read_text(encoding="utf-8")):
        seen = observations.get(native["class"], [])
        rows.append({"class": native["class"], "role": native["role"], "static_source": native["source"],
                     "known_gaps": native.get("known_gaps", []), "native_observations": seen,
                     "constructed_and_retired": any(s.get("construction_inspected") and s.get("retired") for s in seen),
                     "geometry_read_errors": sum("ERROR" in line for s in seen for line in s["geometry_observations"]),
                     "native_combat_parity_verified": False})
    result = {"run": str(run.resolve()), "weapon_classes": len(rows),
              "roles": dict(collections.Counter(r["role"] for r in rows)),
              "constructed_and_retired": sum(r["constructed_and_retired"] for r in rows),
              "construction_observed": sum(bool(r["native_observations"]) for r in rows),
              "native_combat_parity_verified": 0,
              "scope": "Native construction/collider inspection and retirement only; no simulated-contact or combat pass inferred.",
              "weapons": rows}
    output = run / "weapon-inventory-coverage.json"
    output.write_text(json.dumps(result, indent=2), encoding="utf-8")
    print(json.dumps({k: v for k, v in result.items() if k != "weapons"}, indent=2))
    return output


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("run", type=Path)
    parser.add_argument("--manifest", type=Path, default=Path(__file__).resolve().parent.parent /
                        "test-results/dev-feature-checks/weapon-catalogue-audit/coverage-manifest.json")
    args = parser.parse_args()
    analyze(args.run, args.manifest)
