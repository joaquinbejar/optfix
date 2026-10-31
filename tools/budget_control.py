"""Writes a copy of the CU budget with one row's max limit one CU below its measured net max.

Used by `make budget-control`, the failing control of milestone 8 (`docs/PLAN.md`). The
measured net max is the row's max minus the `Noop` median in `docs/benchmarks.md`, the units of
`bench/budget.md`. Standard library only.

    python3 tools/budget_control.py <row> bench/budget.md docs/benchmarks.md
"""

import re
import sys

ROW = re.compile(r"^\| `(.+?)` \| (\d+) \| (\d+) \| (\d+) \| (\d+) \|$")


def main() -> None:
    row, budget, benchmarks = sys.argv[1], sys.argv[2], sys.argv[3]
    measured = {}
    for line in open(benchmarks, encoding="utf-8"):
        m = ROW.match(line.strip())
        if m:
            measured[m.group(1)] = (int(m.group(3)), int(m.group(4)))
    noop_median = measured["Noop"][0]
    limit = measured[row][1] - noop_median - 1
    target = re.compile(r"^\| `" + re.escape(row) + r"` \| (\d+) \| (\d+) \|$")
    hits = 0
    for line in open(budget, encoding="utf-8"):
        m = target.match(line.rstrip("\n"))
        if m:
            hits += 1
            line = f"| `{row}` | {m.group(1)} | {limit} |\n"
        sys.stdout.write(line)
    if hits != 1:
        sys.exit(f"budget_control: {hits} budget rows named {row}")


if __name__ == "__main__":
    main()
