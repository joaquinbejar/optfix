#!/usr/bin/env python3
"""Generates optfix/src/iv_tables.rs: the initial-guess tables of `implied_vol`.

`implied_vol` works on the out-of-the-money option in normalized form (`docs/PLAN.md` 4.6):
`a = |ln(S Dq / K Dr)|`, `s = σ√T` (the total volatility), and `y` the out-of-the-money price
over its upper bound (`S Dq` for a call, `K Dr` for a put), so

    y(a, s) = N(d1) − e^a N(d2),  d1 = −a/s + s/2,  d2 = d1 − s,

increasing in `s` from 0 to 1. With `L = −ln y` and `Z = ln L`, the guess of `ln s` is a
bilinear interpolation in one of two tables:

- near the money (`u = ln a + L <= SWITCH`): `NEAR[row(Z)][u − NEAR_U_MIN]` holds `ln s + L`;
- deep out of the money (`u > SWITCH`): `DEEP[(Z − DEEP_Z_MIN) × 4][w − DEEP_W_MIN]` holds
  `ln s − w`, with `w = ln a − (ln 2 + Z) / 2` (the leading asymptotic `ln s`).

In these coordinates the stored function is smooth and bounded: near the money `ln s + L`
tends to the at-the-money value as `a → 0`, and deep out of the money `ln s ≈ w`. Each node is
the exact root, solved to `10^-12` in `ln s`; the script prints the measured guess error over
a deterministic sample of the supported domain to stderr.

Python standard library only (`decimal` at 150 significant digits):

    python3 tools/gen_iv_tables.py > optfix/src/iv_tables.rs
"""
import math
import random
import sys
from decimal import Decimal as D, getcontext, ROUND_HALF_EVEN

getcontext().prec = 150

# Near table: u in [NEAR_U_MIN, NEAR_U_MAX] step 1; Z rows step 1 on [NEAR_Z_MIN, Z_FINE] and
# step 1/4 on [Z_FINE, Z_MAX].
NEAR_U_MIN, NEAR_U_MAX = -10, 12
NEAR_Z_MIN, Z_FINE, Z_MAX = -28, -4, D("4.25")
# Deep table: w in [DEEP_W_MIN, DEEP_W_MAX] step 1; Z in [DEEP_Z_MIN, Z_MAX] step 1/4.
DEEP_W_MIN, DEEP_W_MAX = -32, 3
DEEP_Z_MIN = 1
# u above which the deep table is used.
SWITCH = 10
# Stored values are at 2^-VALUE_BITS, in i32.
VALUE_BITS = 24


def pi():
    def atan_inv(n):
        x = D(1) / n
        x2 = x * x
        term, total, k = x, D(0), 1
        while abs(term) > D(10) ** -140:
            total += term / k
            term *= -x2
            k += 2
        return total
    return 16 * atan_inv(5) - 4 * atan_inv(239)


PI = pi()
SQRT_2PI = (2 * PI).sqrt()


def lower_tail(t):
    """N(−t) for t >= 0: the series of gen_tables.py below 10, a continued fraction above."""
    if t < 10:
        term, total, k = t, D(0), 1
        while term > D(10) ** -140:
            total += term
            k += 2
            term = term * t * t / k
        return D("0.5") - (-t * t / 2).exp() * total / SQRT_2PI
    # N(−t) = n(t) / (t + 1/(t + 2/(t + 3/(t + ...)))), evaluated from the tail.
    frac = t
    for k in range(400, 0, -1):
        frac = t + k / frac
    return (-t * t / 2).exp() / SQRT_2PI / frac


def cdf(d):
    return lower_tail(-d) if d < 0 else 1 - lower_tail(d)


def y_of(a, s):
    d1 = -a / s + s / 2
    return cdf(d1) - a.exp() * cdf(d1 - s)


def solve_ln_s(a, y):
    """ln s with y(a, s) = y: Newton on ln y(ln s), safeguarded by bisection."""
    lo, hi = D(-150), D(20)
    v = (lo + hi) / 2
    for _ in range(200):
        s = v.exp()
        yv = y_of(a, s)
        if yv <= 0:
            lo = v
            v = (lo + hi) / 2
            continue
        if yv < y:
            lo = v
        else:
            hi = v
        d1 = -a / s + s / 2
        # d ln y / d ln s = s n(d1) / y
        slope = s * (-d1 * d1 / 2).exp() / SQRT_2PI / yv
        step = (y.ln() - yv.ln()) / slope if slope > 0 else D(0)
        nxt = v + step
        if not lo < nxt < hi:
            nxt = (lo + hi) / 2
        if abs(nxt - v) < D(10) ** -13:
            return nxt
        v = nxt
    raise SystemExit(f"no convergence at a = {a}, y = {y}")


def near_z(row):
    fine = row - (Z_FINE - NEAR_Z_MIN)
    return D(NEAR_Z_MIN + row) if fine <= 0 else Z_FINE + D(fine) / 4


NEAR_ROWS = (Z_FINE - NEAR_Z_MIN) + int((Z_MAX - Z_FINE) * 4) + 1
DEEP_ROWS = int((Z_MAX - DEEP_Z_MIN) * 4) + 1


def near_table():
    rows = []
    for j in range(NEAR_ROWS):
        z = near_z(j)
        big_l = z.exp()
        y = (-big_l).exp()
        rows.append([solve_ln_s((D(u) - big_l).exp(), y) + big_l
                     for u in range(NEAR_U_MIN, NEAR_U_MAX + 1)])
    return rows


def deep_table():
    rows = []
    ln2 = D(2).ln()
    for j in range(DEEP_ROWS):
        z = DEEP_Z_MIN + D(j) / 4
        y = (-z.exp()).exp()
        rows.append([solve_ln_s((D(w) + (ln2 + z) / 2).exp(), y) - w
                     for w in range(DEEP_W_MIN, DEEP_W_MAX + 1)])
    return rows


def interp(table, col, row):
    """Bilinear interpolation at fractional (col, row), clamped to the table."""
    col = min(max(col, 0.0), len(table[0]) - 1.0)
    row = min(max(row, 0.0), len(table) - 1.0)
    i = min(int(col), len(table[0]) - 2)
    j = min(int(row), len(table) - 2)
    fu, fv = col - i, row - j
    t = lambda r, c: float(table[r][c])
    return ((1 - fu) * (1 - fv) * t(j, i) + fu * (1 - fv) * t(j, i + 1)
            + (1 - fu) * fv * t(j + 1, i) + fu * fv * t(j + 1, i + 1))


def guess_ln_s(near, deep, a, big_l):
    z = math.log(big_l)
    la = math.log(a) if a > 0 else -1e9
    u = la + big_l
    if u > SWITCH:
        w = la - (math.log(2) + z) / 2
        return interp(deep, w - DEEP_W_MIN, (z - DEEP_Z_MIN) * 4) + w
    fine = Z_FINE - NEAR_Z_MIN
    row = z - NEAR_Z_MIN if z <= Z_FINE else fine + (z - Z_FINE) * 4
    return interp(near, u - NEAR_U_MIN, row) - big_l


def check(near, deep, n=3000):
    """Guess error in ln s over the supported domain: a in [0, 21.2], s in [1.2e-12, 27.4],
    d1 in [−8, 7] (beyond, vega is below `implied_vol`'s threshold), y >= 10^-30."""
    g = random.Random(0x1f)
    errs = []
    while len(errs) < n:
        a = 0.0 if g.random() < 0.05 else 10 ** (-14 + 15.33 * g.random())
        d1 = -8 + 15 * g.random()
        r = math.sqrt(d1 * d1 + 2 * a)
        s = d1 + r if d1 >= 0 else 2 * a / (r - d1)
        if not 1.2e-12 <= s <= 27.4:
            continue
        y = y_of(D(a), D(s))
        if y < D(10) ** -30:
            continue
        big_l = float(-y.ln())
        errs.append(abs(guess_ln_s(near, deep, a, big_l) - math.log(s)))
    errs.sort()
    print(f"guess |error| in ln s over {n} points: median {errs[n // 2]:.2e}, "
          f"p99 {errs[int(n * 0.99)]:.2e}, max {errs[-1]:.2e}", file=sys.stderr)


def fmt(name, rows, doc):
    out = [f"/// {doc}", "#[rustfmt::skip]", f"pub(crate) const {name}: [[i32; {len(rows[0])}]; {len(rows)}] = ["]
    for r in rows:
        vals = [int((v * (1 << VALUE_BITS)).to_integral_value(ROUND_HALF_EVEN)) for v in r]
        if any(abs(v) >= 1 << 31 for v in vals):
            raise SystemExit(f"{name}: a value does not fit i32")
        out.append("    [" + ", ".join(str(v) for v in vals) + "],")
    out.append("];")
    return "\n".join(out)


def main():
    near, deep = near_table(), deep_table()
    check(near, deep)
    print("\n".join([
        "//! Initial-guess tables of `implied_vol`. Generated by `tools/gen_iv_tables.py`; do not",
        "//! edit by hand. Values are at `2^-24`, rounded to nearest.",
        "",
        f"/// `u = ln a + L` of the first `NEAR` column (columns step 1).",
        f"pub(crate) const NEAR_U_MIN: i32 = {NEAR_U_MIN};",
        f"/// `Z = ln L` of the first `NEAR` row; rows step 1 up to `Z_FINE`, then 1/4.",
        f"pub(crate) const NEAR_Z_MIN: i32 = {NEAR_Z_MIN};",
        f"/// `Z` where the `NEAR` rows switch from step 1 to step 1/4.",
        f"pub(crate) const Z_FINE: i32 = {Z_FINE};",
        f"/// `u` above which the guess comes from `DEEP`.",
        f"pub(crate) const SWITCH: i32 = {SWITCH};",
        f"/// `w = ln a − (ln 2 + Z) / 2` of the first `DEEP` column (columns step 1).",
        f"pub(crate) const DEEP_W_MIN: i32 = {DEEP_W_MIN};",
        f"/// `Z` of the first `DEEP` row (rows step 1/4).",
        f"pub(crate) const DEEP_Z_MIN: i32 = {DEEP_Z_MIN};",
        "",
        fmt("NEAR", near, "`ln s + L` at `(Z, u)`: the near-the-money guess."),
        "",
        fmt("DEEP", deep, "`ln s − w` at `(Z, w)`: the deep out-of-the-money guess."),
    ]))


if __name__ == "__main__":
    main()
