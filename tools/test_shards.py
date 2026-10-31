"""Splits `make test` into shards for CI (docs/PLAN.md section 8, milestone 9).

    python3 tools/test_shards.py <shards> <shard>

prints the `cargo test` target selectors of shard `<shard>` (0-based). The integration test
targets come from `cargo metadata`, so a new test file is always in exactly one shard; shard 0
also takes the library and binary unit tests (`--lib --bins`). Doctests are run by
`make test-shard` on shard 0. Every shard runs its targets with their full case counts, so
the union of the shards is exactly `make test`.

Targets are assigned longest first to the least loaded shard, by the durations measured on
the CI runner (minutes, debug profile, 2026-10-31); an unlisted target counts as one minute.
Standard library only.
"""

import json
import subprocess
import sys

MINUTES = {
    "bsm_props": 8.0,
    "cu": 6.9,
    "norm_props": 4.7,
    "fixed_props": 2.7,
    "exp_ln_props": 2.0,
    "precision": 1.5,
    "wide_props": 0.7,
    "iv_internals": 0.6,
    "iv_round_trip": 0.5,
    "bsm_oracle": 0.4,
    "bsm_fixture": 0.3,
}


def test_targets() -> list[str]:
    out = subprocess.run(
        ["cargo", "metadata", "--no-deps", "--format-version", "1"],
        check=True,
        capture_output=True,
        text=True,
    ).stdout
    names = []
    for package in json.loads(out)["packages"]:
        for target in package["targets"]:
            if target["kind"] == ["test"]:
                names.append(target["name"])
    if len(names) != len(set(names)):
        sys.exit("test_shards: two packages share a test target name")
    return sorted(names)


def assign(names: list[str], shards: int) -> list[list[str]]:
    load = [0.0] * shards
    out: list[list[str]] = [[] for _ in range(shards)]
    # Shard 0 also runs the unit tests and doctests: about one minute.
    load[0] = 1.0
    for name in sorted(names, key=lambda n: (-MINUTES.get(n, 1.0), n)):
        i = min(range(shards), key=lambda k: (load[k], k))
        out[i].append(name)
        load[i] += MINUTES.get(name, 1.0)
    return out


def main() -> None:
    shards, shard = int(sys.argv[1]), int(sys.argv[2])
    if not 0 <= shard < shards:
        sys.exit(f"test_shards: shard {shard} is not in [0, {shards})")
    names = test_targets()
    plan = assign(names, shards)
    if sorted(n for s in plan for n in s) != names:
        sys.exit("test_shards: the shards do not cover every test target exactly once")
    args = [f"--test {n}" for n in plan[shard]]
    if shard == 0:
        args = ["--lib", "--bins"] + args
    print(" ".join(args))


if __name__ == "__main__":
    main()
