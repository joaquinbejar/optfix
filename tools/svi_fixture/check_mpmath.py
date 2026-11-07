"""Independent 100-digit mpmath checker for the SVI functions (docs/PLAN-phase2.md, milestone
22). Reads the outputs that `OPTFIX_FIXTURE_EXPORT=<dir> cargo test -p optfix --release --test
svi_fixture` writes, recomputes every reference value from the raw inputs (ignoring the
fixture's own columns) and counts violations of the documented contracts:

- `svi_w_out.csv`: `lo <= w(k) <= hi` for `total_variance_bounds` at the exact `k`;
- `svi_e2e_out.csv`: an accepted slice has a true minimum of at least 0 and
  `min_total_variance` at most that; `|k̂ − k*| < 5` ulps; `lo <= sigma* <= hi` and
  `lo <= svi_vol <= hi`; the premium at `sigma*` inside `svi_price_bounds`.

    python check_mpmath.py EXPORT_DIR RESULT.json
"""
import csv
import json
import sys

import mpmath as mp

mp.mp.dps = 100
ONE = mp.mpf(2) ** 64


def real(v):
    return mp.mpf(int(v)) / ONE


def w_of(p, k):
    a, b, rho, m, s = p
    x = k - m
    return a + b * (rho * x + mp.sqrt(x * x + s * s))


def premium(call, s, k, t, sig, r, q):
    a = sig * mp.sqrt(t)
    d1 = (mp.log(s / k) + (r - q + sig * sig / 2) * t) / a
    d2 = d1 - a
    dq, dr = mp.exp(-q * t), mp.exp(-r * t)
    if call:
        return s * dq * mp.ncdf(d1) - k * dr * mp.ncdf(d2)
    return k * dr * mp.ncdf(-d2) - s * dq * mp.ncdf(-d1)


def main():
    out_dir, result = sys.argv[1], sys.argv[2]
    names = ["w", "minimum", "k", "sigma*", "svi_vol", "premium"]
    violations = {n: 0 for n in names}
    checked = {n: 0 for n in names}
    first = {}
    worst_k = mp.mpf(0)

    def fail(name, row, detail):
        violations[name] += 1
        first.setdefault(name, {"row": row, "detail": detail})

    for row in csv.reader(open(f"{out_dir}/svi_w_out.csv")):
        p = [real(v) for v in row[:5]]
        w = w_of(p, real(row[5]))
        lo, hi = real(row[7]), real(row[8])
        checked["w"] += 1
        if not lo <= w <= hi:
            fail("w", row, mp.nstr(w, 40))

    refused = 0
    for row in csv.reader(open(f"{out_dir}/svi_e2e_out.csv")):
        if row[16] == "REFUSED":
            refused += 1
            continue
        p = [real(v) for v in row[:5]]
        s, k, t, r, q = [real(v) for v in row[5:10]]
        call = row[10] == "0"
        true_min = p[0] + p[1] * p[4] * mp.sqrt(1 - p[2] * p[2])
        checked["minimum"] += 1
        if true_min < 0 or real(row[16]) > true_min:
            fail("minimum", row, mp.nstr(true_min, 40))
        ks = mp.log(k / s) - (r - q) * t
        kerr = abs(real(row[17]) - ks) * ONE
        worst_k = max(worst_k, kerr)
        checked["k"] += 1
        if kerr >= 5:
            fail("k", row, mp.nstr(kerr, 20))
        sig = mp.sqrt(w_of(p, ks) / t)
        if not row[18].startswith("ERR"):
            lo, hi = real(row[18]), real(row[19])
            checked["sigma*"] += 1
            if not lo <= sig <= hi:
                fail("sigma*", row, mp.nstr(sig, 40))
            if not row[20].startswith("ERR"):
                checked["svi_vol"] += 1
                if not lo <= real(row[20]) <= hi:
                    fail("svi_vol", row, row[20])
        if not row[21].startswith("ERR"):
            want = premium(call, s, k, t, sig, r, q)
            checked["premium"] += 1
            if not real(row[21]) <= want <= real(row[22]):
                fail("premium", row, mp.nstr(want, 40))

    print("checked", checked)
    print("violations", violations)
    print("refused slices", refused, "worst |k_hat - k*| ulps", mp.nstr(worst_k, 6))
    json.dump(
        {
            "checked": checked,
            "violations": violations,
            "refused": refused,
            "worst_k_ulps": mp.nstr(worst_k, 6),
            "first": first,
        },
        open(result, "w"),
        indent=2,
    )


if __name__ == "__main__":
    main()
