#!/usr/bin/env python3
"""Generates optfix/src/kernel/tables.rs: the integer constants of the exp and ln kernels.

The polynomial coefficients interpolate the target function at Chebyshev nodes of the reduced
interval, which is within a small factor of the minimax polynomial of the same degree.

Python standard library only (`decimal` at 100 significant digits; its exp and ln are
correctly rounded), so the output is reproducible anywhere:

    python3 tools/gen_tables.py > optfix/src/kernel/tables.rs

Every constant is the exact real value rounded to nearest (ties to even) at the stated scale.
"""
from decimal import Decimal as D, getcontext, ROUND_HALF_EVEN
from math import factorial

getcontext().prec = 120

LN2 = D(2).ln()

# exp: x = (64 k + j) ln2/64 + r, |r| <= ln2/128; exp(x) = 2^k 2^(j/64) e^r.
EXP_TABLE_BITS = 6
# e^r = 1 + r + r^2 p(r) with p of degree EXP_DEGREE − 2 fitted to (e^r − 1 − r) / r^2.
EXP_DEGREE = 6
# ln: m in [1, 2), j = round((m - 1) 128), c_j = 1 + j/128, z = m / c_j - 1, |z| <= 2^-8.
LN_TABLE_BITS = 7
# ln(1 + z) = z − z^2 q(z) with q of degree LN_DEGREE − 2 fitted to (z − ln(1 + z)) / z^2.
LN_DEGREE = 7
# N(−a) = e^(−a²/2) P(a) for a >= 0, P piecewise polynomial of degree NORM_DEGREE on
# segments of width 2^-NORM_TABLE_BITS covering [0, NORM_SEGMENTS / 4).
NORM_TABLE_BITS = 2
NORM_SEGMENTS = 37
NORM_DEGREE = 12
# Fit intervals: |r| <= ln2/128 and |z| <= 2^-8, widened slightly for the estimated reduction
# index and the rounded reciprocals.
FIT_MARGIN = D(1) + D(2) ** -10


def pi():
    # Machin: pi = 16 atan(1/5) − 4 atan(1/239).
    def atan_inv(n):
        x = D(1) / n
        x2 = x * x
        term, total, k = x, D(0), 1
        while abs(term) > D(10) ** -110:
            total += term / k
            term *= -x2
            k += 2
        return total
    return 16 * atan_inv(5) - 4 * atan_inv(239)


def cos(x):
    term, total, k = D(1), D(0), 0
    while abs(term) > D(10) ** -110:
        total += term
        term *= -x * x / ((k + 1) * (k + 2))
        k += 2
    return total


def series(coef, x):
    """Sum of coef(i) x^i for i >= 0 until the terms vanish."""
    total, xi, i = D(0), D(1), 0
    while True:
        t = coef(i) * xi
        if t == 0 or (i > 5 and abs(t) < D(10) ** -110):
            return total
        total += t
        xi *= x
        i += 1


def solve(a, b):
    """Gaussian elimination with partial pivoting."""
    n = len(b)
    m = [row[:] + [b[i]] for i, row in enumerate(a)]
    for c in range(n):
        p = max(range(c, n), key=lambda r: abs(m[r][c]))
        m[c], m[p] = m[p], m[c]
        for r in range(n):
            if r != c:
                f = m[r][c] / m[c][c]
                m[r] = [x - f * y for x, y in zip(m[r], m[c])]
    return [m[i][n] / m[i][i] for i in range(n)]


def cheb_fit(f, degree, half_width):
    """Polynomial of `degree` interpolating f at the Chebyshev nodes of [−w, w]."""
    n = degree + 1
    nodes = [half_width * cos((2 * k + 1) * pi() / (2 * n)) for k in range(n)]
    return solve([[x ** i for i in range(n)] for x in nodes], [f(x) for x in nodes])


def norm_p(a):
    """P(a) = N(−a) e^(a²/2) = e^(a²/2) / 2 − S(a) / √(2π), S(a) = Σ a^(2n+1) / (2n+1)!!.

    The series has only positive terms; at a = 9.25 the subtraction cancels about 20 digits of
    the 120 in the context.
    """
    term, total, k = a, D(0), 1
    while term > D(10) ** -110:
        total += term
        k += 2
        term = term * a * a / k
    return (a * a / 2).exp() / 2 - total / (2 * pi()).sqrt()


def cheb_fit_on(f, degree, lo, hi):
    """Polynomial in t = x − centre interpolating f at the Chebyshev nodes of [lo, hi]."""
    centre, half = (lo + hi) / 2, (hi - lo) / 2
    n = degree + 1
    ts = [half * cos((2 * k + 1) * pi() / (2 * n)) for k in range(n)]

    def powers(t):
        out, v = [], D(1)
        for _ in range(n):
            out.append(v)
            v *= t
        return out

    return solve([powers(t) for t in ts], [f(centre + t) for t in ts])


def rnd(x):
    return int(x.to_integral_value(ROUND_HALF_EVEN))


def scaled(x, bits):
    return rnd(x * (D(2) ** bits))


def array(name, ty, values, doc):
    lines = [f"/// {doc}", f"pub(crate) const {name}: [{ty}; {len(values)}] = ["]
    lines += [f"    {v:#x}," for v in values]
    lines.append("];")
    return "\n".join(lines)


def const(name, ty, value, doc):
    return f"/// {doc}\npub(crate) const {name}: {ty} = {value:#x};"


def main():
    nb = 1 << EXP_TABLE_BITS
    exp_l = LN2 / nb
    exp_table = [scaled(D(2) ** (D(j) / nb), 63) for j in range(nb)]
    # (e^r − 1 − r) / r^2 = sum r^i / (i + 2)!.
    exp_p = cheb_fit(lambda r: series(lambda i: D(1) / factorial(i + 2), r),
                     EXP_DEGREE - 2, LN2 / (2 * nb) * FIT_MARGIN)
    exp_coef = [scaled(c, 64) for c in exp_p]

    lb = 1 << LN_TABLE_BITS
    ln_rinv = [scaled(D(1) / (1 + D(j) / lb), 63) for j in range(lb + 1)]
    # The table stores -ln of the rounded reciprocal, so m * RINV_j is the exact argument.
    ln_table = [scaled(-(D(r) / D(2) ** 63).ln(), 128) for r in ln_rinv]
    # (z − ln(1 + z)) / z^2 = sum (−1)^i z^i / (i + 2); Horner subtracts, so the stored
    # coefficient of z^i is (−1)^i a_i, all positive.
    ln_q = cheb_fit(lambda z: series(lambda i: D((-1) ** i) / (i + 2), z),
                    LN_DEGREE - 2, D(2) ** -LN_TABLE_BITS / 2 * FIT_MARGIN)
    ln_coef = [scaled(c * (-1) ** i, 64) for i, c in enumerate(ln_q)]

    out = [
        "//! Constants of the `exp` and `ln` kernels. Generated by `tools/gen_tables.py`; do not",
        "//! edit by hand. Each value is the exact constant rounded to nearest at the stated scale.",
        "",
        f"/// `log2` of the number of `exp` table entries.",
        f"pub(crate) const EXP_TABLE_BITS: u32 = {EXP_TABLE_BITS};",
        f"/// `log2` of the number of `ln` buckets.",
        f"pub(crate) const LN_TABLE_BITS: u32 = {LN_TABLE_BITS};",
        "",
        const("LN2_Q120", "u128", scaled(LN2, 120), "`ln 2 × 2^120`."),
        "",
        const("EXP_INV_STEP_Q25", "i64", scaled(nb / LN2, 25),
              f"`{nb} / ln 2 × 2^25`: estimates the binary reduction index."),
        "",
        const("EXP_STEP_HI_Q64", "u64", rnd(exp_l * D(2) ** 64),
              f"`ln 2 / {nb}` in units of `2^-64`, rounded: the high part of the binary "
              "reduction step."),
        "",
        const("EXP_STEP_LO_Q114", "i64", rnd((exp_l * D(2) ** 64 - rnd(exp_l * D(2) ** 64)) * D(2) ** 50),
              "The rest of the step, `ln 2 / 64 − EXP_STEP_HI_Q64 / 2^64`, in units of `2^-114`."),
        "",
        array("EXP_TABLE", "u64", exp_table, f"`2^(j/{nb}) × 2^63` for `j` in `0..{nb}`."),
        "",
        array("EXP_COEF", "u64", exp_coef,
              f"`p(r) = (e^r − 1 − r) / r^2 ≈ Σ EXP_COEF[i] r^i / 2^64` on `|r| <= ln2/{2 * nb}` "
              f"(Taylor: `1/(i + 2)!`)."),
        "",
        array("LN_RINV", "u64", ln_rinv,
              f"`1 / (1 + j/{lb}) × 2^63` for `j` in `0..={lb}`."),
        "",
        array("LN_TABLE", "u128", ln_table,
              "`−ln(LN_RINV[j] / 2^63) × 2^128`: exact for the rounded reciprocal."),
        "",
        array("LN_COEF", "u64", ln_coef,
              f"`q(z) = (z − ln(1 + z)) / z^2 ≈ Σ (−1)^i LN_COEF[i] z^i / 2^64` on `|z| <= "
              f"2^-{LN_TABLE_BITS + 1}` (Taylor: `1/(i + 2)`)."),
        "",
        f"/// `log2` of the number of normal-CDF segments per unit: segments of width "
        f"`1/{1 << NORM_TABLE_BITS}`.",
        f"pub(crate) const NORM_TABLE_BITS: u32 = {NORM_TABLE_BITS};",
        "",
        const("INV_SQRT_2PI_Q128", "u128", scaled(1 / (2 * pi()).sqrt(), 128), "`1 / √(2π) × 2^128`."),
        "",
        norm_table(),
    ]
    print("\n".join(out))


def norm_table():
    w = D(1) / (1 << NORM_TABLE_BITS)
    b0, rows = [], []
    for i in range(NORM_SEGMENTS):
        coef = cheb_fit_on(norm_p, NORM_DEGREE, i * w, (i + 1) * w)
        # Signs alternate (+, −, +, ...) on every segment; store magnitudes.
        for k, c in enumerate(coef):
            if (c > 0) != (k % 2 == 0):
                raise SystemExit(f"segment {i}: coefficient {k} breaks the sign pattern")
        b0.append(f"    {scaled(coef[0], 128):#x},")
        rows.append("    [" + ", ".join(f"{scaled(abs(c), 64):#x}" for c in coef[1:]) + "],")
    seg = (f"`a` in `[i/{1 << NORM_TABLE_BITS}, (i + 1)/{1 << NORM_TABLE_BITS})`, "
           f"`t = a − (i + 1/2)/{1 << NORM_TABLE_BITS}`")
    return "\n".join(
        [f"/// `P(a) = N(−a) e^(a²/2) ≈ NORM_B0[i] / 2^128 − t Σ (−1)^k NORM_COEF[i][k] t^k / 2^64` "
         f"on segment `i`, {seg}.",
         f"pub(crate) const NORM_COEF: [[u64; {NORM_DEGREE}]; {NORM_SEGMENTS}] = ["]
        + rows + ["];", "",
                  "/// The constant terms of the segment polynomials, at `2^128`.",
                  f"pub(crate) const NORM_B0: [u128; {NORM_SEGMENTS}] = ["] + b0 + ["];"])


if __name__ == "__main__":
    main()
