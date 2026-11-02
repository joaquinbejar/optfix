"""Compares a downstream CU table measured with optfix's merge base against the same table
measured with the PR head (docs/PLAN-improvements.md, milestone 15).

    python3 tools/downstream_cu.py BASE.md HEAD.md [SUMMARY.md]

Each file is a downstream `docs/benchmarks.md` (varswap's layout: one markdown table whose
first column names the scenario). The two tables must have the same header and the same rows
in the same order, with the same text in every non-numeric column; otherwise the measurements
are not comparable and the check fails. It then fails if any numeric cell of the head table is
greater than the base's, and reports every changed cell either way. With SUMMARY.md the report
is also appended there (the job summary).
"""
import sys


def table(path):
    """The first markdown table of `path`: (header, rows), each a list of stripped cells."""
    rows = []
    for line in open(path, encoding="utf-8"):
        line = line.strip()
        if line.startswith("|"):
            cells = [c.strip() for c in line.strip("|").split("|")]
            if all(set(c) <= set("-: ") for c in cells):
                continue
            rows.append(cells)
        elif rows:
            break
    if len(rows) < 2:
        sys.exit(f"downstream-cu: no table in {path}")
    return rows[0], rows[1:]


def number(cell):
    """`cell` as an integer, or None if it is not one (thousands separators allowed)."""
    plain = cell.replace(",", "")
    return int(plain) if plain.isdigit() else None


def compare(base, head):
    """(errors, changes) for two (header, rows) tables."""
    errors, changes = [], []
    (bh, brows), (hh, hrows) = base, head
    if bh != hh:
        return [f"headers differ: {bh} against {hh}"], changes
    if [r[0] for r in brows] != [r[0] for r in hrows]:
        return [f"rows differ: {[r[0] for r in brows]} against {[r[0] for r in hrows]}"], changes
    for b, h in zip(brows, hrows):
        if len(b) != len(h) or len(b) != len(bh):
            errors.append(f"{b[0]}: {len(b)} cells against {len(h)}")
            continue
        for col, bc, hc in zip(bh, b, h):
            bn, hn = number(bc), number(hc)
            if (bn is None) != (hn is None):
                errors.append(f"{b[0]} / {col}: '{bc}' against '{hc}'")
            elif bn is None:
                if bc != hc:
                    errors.append(f"{b[0]} / {col}: '{bc}' against '{hc}'")
            elif hn != bn:
                grew = hn > bn
                changes.append((grew, f"{b[0]} / {col}: {bn} → {hn} ({hn - bn:+d})"))
                if grew:
                    errors.append(f"{b[0]} / {col} grew: {bn} → {hn} ({hn - bn:+d})")
    return errors, changes


def main():
    if len(sys.argv) not in (3, 4):
        sys.exit(__doc__)
    errors, changes = compare(table(sys.argv[1]), table(sys.argv[2]))
    lines = ["Downstream CU, merge base against head:"]
    lines += [f"- {text}" for _, text in changes] or ["- no change"]
    lines += [f"- FAIL: {e}" for e in errors]
    report = "\n".join(lines) + "\n"
    print(report, end="")
    if len(sys.argv) == 4:
        with open(sys.argv[3], "a", encoding="utf-8") as f:
            f.write(report)
    sys.exit(1 if errors else 0)


if __name__ == "__main__":
    main()
