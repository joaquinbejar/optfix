"""Writes the committed SVI fixtures of docs/PLAN-phase2.md, milestone 22, with mpmath at 80
digits. Needs mpmath (a local dev venv; not a crate dependency).

    python gen_fixture.py W_OUT.csv E2E_OUT.csv

W_OUT (`optfix/tests/data/svi_w.csv`): raw Q64.64 `a, b, rho, m, sigma, k`, then
`nint(w(k) × 2^96)`: w at the exact argument, with 32 bits below the ulp. 3,000 rows in each
of four strata: uniform over the domain; near k = m with sigma <= 1e-3; the far wings with
1 − |rho| <= 1e-2, cancelling; around max(|x|, sigma) = 1 and the domain edges.

E2E_OUT (`optfix/tests/data/svi_e2e.csv`): raw `a, b, rho, m, sigma, S, K, T, r, q`, the kind
(0 call, 1 put), then `nint(v × 2^96)` of `k* = ln(K/S) − (r − q) T`, `w(k*)`,
`sigma* = sqrt(w(k*) / T)`, the Black-Scholes-Merton premium at `sigma*` and the true minimum
`a + b sigma sqrt(1 − rho^2)`. 6,000 market rows (a slice whose minimum is a volatility in
[0.01, 2] at T) and 1,000 rows whose slice has |rho| within a few ulps to 2^-1 of 1 and the
`a` at which rho^2 rounded down would put the minimum's bound at exactly 0.
"""
import math
import random
import sys

import mpmath as mp

mp.mp.dps = 80
ONE = 1 << 64
X96 = mp.mpf(2) ** 96
SPOT_MIN, SPOT_MAX = 18446744073709, 10**9 * ONE
RATE_MIN, RATE_MAX = -1844674407370955162, ONE // 2
SIGMA_MIN = 18446744073709
rng = random.Random(20261106)


def raw(v):
    return int(mp.floor(mp.mpf(v) * ONE))


def logu(lo, hi):
    return math.exp(rng.uniform(math.log(lo), math.log(hi)))


def sign():
    return 1 if rng.random() < 0.5 else -1


def real(r):
    return mp.mpf(r) / ONE


def w_exact(p, k):
    a, b, rho, m, s = map(real, p)
    x = k - m
    return a + b * (rho * x + mp.sqrt(x * x + s * s))


def min_exact(p):
    a, b, rho, _, s = map(real, p)
    return a + b * s * mp.sqrt(1 - rho * rho)


def a_for(b, rho, s, margin):
    part = int(mp.floor(min_exact([0, b, rho, 0, s]) * ONE))
    return max(-1000 * ONE, min(1000 * ONE, margin - part))


def sample(st):
    b = raw(logu(1e-4, 10))
    if rng.randrange(64) == 0:
        b = 0
    s = raw(logu(1e-6, 10))
    m = raw(rng.uniform(-10, 10))
    rho = rng.randrange(-ONE + 1, ONE)
    k = rng.randrange(-32 * ONE, 32 * ONE + 1)
    if st == 1:
        s = raw(logu(1e-6, 1e-3))
        k = m + sign() * raw(logu(1e-12, 1e-2))
    elif st == 2:
        gap = max(1, raw(logu(1 / ONE, 1e-2)))
        rho = -(1 if k >= m else -1) * (ONE - gap)
    elif st == 3:
        c = rng.randrange(6)
        if c == 0:
            d = sign() * raw(logu(1e-18, 1e-2))
            k = max(-32 * ONE, min(32 * ONE, m + sign() * (ONE + d)))
        elif c == 1:
            s = max(1, ONE + sign() * raw(logu(1e-18, 1e-2)))
        elif c == 2:
            b, s = 10 * ONE, 10 * ONE
        elif c == 3:
            s = SIGMA_MIN
        elif c == 4:
            m = sign() * 10 * ONE
        else:
            k = sign() * 32 * ONE
    margin = rng.randrange(0, 1 << 20) if rng.randrange(4) == 0 else raw(logu(1e-6, 100))
    return [a_for(b, rho, s, margin), b, rho, m, s], k


def market():
    s = max(SPOT_MIN, min(SPOT_MAX, raw(logu(1e-6, 1e9))))
    t = logu(1e-8, 1e-3) if rng.randrange(8) == 0 else logu(1e-3, 30)
    t = max(1, min(30 * ONE, raw(t)))
    r = max(RATE_MIN, min(RATE_MAX, raw(rng.uniform(-0.1, 0.5))))
    q = 0 if rng.randrange(3) == 0 else max(RATE_MIN, min(RATE_MAX, raw(rng.uniform(-0.1, 0.5))))
    lk = rng.uniform(-2.99, 2.99)
    k = int(mp.floor(mp.mpf(s) * mp.exp(lk)))
    k = max(s // 20 + 1, min(s * 20, k))
    k = max(SPOT_MIN, min(SPOT_MAX, k))
    return s, k, t, r, q


def market_slice(t):
    p, _ = sample(rng.randrange(4))
    v0 = logu(0.01, 2)
    level = max(1, raw(mp.mpf(v0) ** 2 * real(t)))
    tf = min(1.0, t / ONE)
    if p[1]:
        p[1] = max(1, int(p[1] * tf))
    p[0] = a_for(p[1], p[2], p[4], level)
    return p


def mul_down(x, y):
    return (x * y) >> 64


def isqrt_down(v):
    return math.isqrt(v << 64)


def attacked_slice():
    p, _ = sample(rng.randrange(4))
    gap = 1 << rng.randrange(63)
    gap = min(ONE // 2, gap + rng.randrange(gap))
    rho = ONE - gap if rng.random() < 0.5 else gap - ONE
    b = max(1, p[1])
    root = isqrt_down(ONE - mul_down(rho, rho))
    p[0], p[1], p[2] = -mul_down(mul_down(b, p[4]), root), b, rho
    return p


def nint96(v):
    return int(mp.nint(v * X96))


def bsm(call, s, k, t, sig, r, q):
    s, k, t, r, q = map(real, (s, k, t, r, q))
    a = sig * mp.sqrt(t)
    d1 = (mp.log(s / k) + (r - q + sig * sig / 2) * t) / a
    d2 = d1 - a
    dq, dr = mp.exp(-q * t), mp.exp(-r * t)
    if call:
        return s * dq * mp.ncdf(d1) - k * dr * mp.ncdf(d2)
    return k * dr * mp.ncdf(-d2) - s * dq * mp.ncdf(-d1)


def e2e_row(p):
    s, k, t, r, q = market()
    if p is None:
        p = market_slice(t)
    kind = rng.randrange(2)
    ks = mp.log(real(k) / real(s)) - (real(r) - real(q)) * real(t)
    w = w_exact(p, ks)
    sig = mp.sqrt(w / real(t)) if w >= 0 else mp.mpf(0)
    price = bsm(kind == 0, s, k, t, sig, r, q) if sig > 0 else mp.mpf(0)
    outs = [ks, w, sig, price, min_exact(p)]
    return p + [s, k, t, r, q, kind] + [nint96(v) for v in outs]


def main():
    w_rows = []
    for st in range(4):
        for _ in range(3000):
            p, k = sample(st)
            w_rows.append(p + [k, nint96(w_exact(p, real(k)))])
    open(sys.argv[1], "w").write("".join(",".join(map(str, r)) + "\n" for r in w_rows))
    e_rows = [e2e_row(None) for _ in range(6000)]
    e_rows += [e2e_row(attacked_slice()) for _ in range(1000)]
    open(sys.argv[2], "w").write("".join(",".join(map(str, r)) + "\n" for r in e_rows))
    print(len(w_rows), len(e_rows))


if __name__ == "__main__":
    main()
