"""Writes the committed fixture of the SVI no-arbitrage checks (docs/PLAN-phase2.md, milestone
23) with mpmath at 80 digits. Needs mpmath (a local dev venv; not a crate dependency).

    python gen_arb_fixture.py G_OUT.csv

G_OUT (`optfix/tests/data/svi_g.csv`): a group tag, raw Q64.64 `a, b, rho, m, sigma, k`, then
`nint(w(k) × 2^96)` and `nint(g(k) × 2^96)`, Durrleman's
`g = (1 − k w' / (2 w))² − (w'² / 4) (1 / w + 1 / 4) + w'' / 2` at the exact argument (`-` when
`w(k) <= 0`). Groups:

- `vogt`: the slice of Gatheral and Jacquier (2014), example 3.1, on the grid `[−2, 2]` with 401
  nodes (`SviGrid`'s node formula); the script prints where mpmath's `g` is negative.
- `clean<i>`: six slices that must pass, on `[−3, 3]` with 601 nodes: four SSVI slices
  (Gatheral and Jacquier 2014, theorem 4.2: `θ φ (1 + |ρ|) < 4` and `θ φ² (1 + |ρ|) <= 4`, in
  raw form `a = θ (1 − ρ²) / 2`, `b = θ φ / 2`, `m = −ρ / φ`, `σ = sqrt(1 − ρ²) / φ`) and two
  symmetric slices with a small `b`. The script asserts mpmath's `g > 0` at every node and the
  wing condition.
- `rand`: 6,000 nodes of random slices near the market's range (`a` in `[−0.1, 0.1]`, `b` in
  `[0, 0.6]`, `|ρ| <= 0.95`, `|m| <= 0.5`, `σ` in `[0.01, 1]`, a positive minimum), `k` within
  3 of `m`.
- `edge`: 2,000 nodes within `10^-19` to `10^-2` of a sign change of `g` (bisected at 80
  digits) on random slices of the same kind, the nodes where the check's band matters.
"""
import random
import sys

import mpmath as mp

mp.mp.dps = 80
ONE = 1 << 64
X96 = mp.mpf(2) ** 96
rng = random.Random(20261107)


def fx(num, den):
    """`Fixed::from_ratio(num, den, Rounding::Down)`."""
    return (num << 64) // den


def raw(v):
    return int(mp.floor(mp.mpf(v) * ONE))


def real(r):
    return mp.mpf(r) / ONE


def w_and_g(p, kr):
    a, b, rho, m, s = map(real, p)
    k = real(kr)
    x = k - m
    r = mp.sqrt(x * x + s * s)
    w = a + b * (rho * x + r)
    if w <= 0:
        return w, None
    w1 = b * (rho + x / r)
    w2 = b * s * s / r**3
    g = (1 - k * w1 / (2 * w)) ** 2 - w1 * w1 / 4 * (1 / w + mp.mpf(1) / 4) + w2 / 2
    return w, g


def sign():
    return 1 if rng.random() < 0.5 else -1


def random_slice():
    """A slice near the market's range with a minimum above 10^-4, or None."""
    p = [
        raw(rng.uniform(-0.1, 0.1)),
        raw(rng.uniform(0, 0.6)),
        raw(rng.uniform(-0.95, 0.95)),
        raw(rng.uniform(-0.5, 0.5)),
        raw(rng.uniform(0.01, 1)),
    ]
    a, b, rho, _, s = map(real, p)
    if a + b * s * mp.sqrt(1 - rho * rho) <= mp.mpf("1e-4"):
        return None
    return p


def nodes(lo, hi, n):
    lo, hi = lo * ONE, hi * ONE
    return [lo + (hi - lo) * i // (n - 1) for i in range(n)]


def nint(v):
    return str(int(mp.nint(v * X96)))


def row(tag, p, k):
    w, g = w_and_g(p, k)
    return [tag, *map(str, p), str(k), nint(w), "-" if g is None else nint(g)], g


def main():
    out = []
    vogt = [fx(-410, 10000), fx(1331, 10000), fx(3060, 10000), fx(3586, 10000), fx(4153, 10000)]
    neg = []
    for k in nodes(-2, 2, 401):
        r, g = row("vogt", vogt, k)
        out.append(r)
        if g < 0:
            neg.append((k, g))
    gmin = min(g for _, g in neg)
    print(
        f"vogt: g < 0 at {len(neg)} nodes, k in [{mp.nstr(real(neg[0][0]), 4)}, "
        f"{mp.nstr(real(neg[-1][0]), 4)}], min g {mp.nstr(gmin, 6)}"
    )

    clean = []
    for theta, phi, rho in [
        (mp.mpf("0.04"), 5, mp.mpf("-0.7")),
        (mp.mpf("0.25"), 2, mp.mpf("-0.5")),
        (mp.mpf(1), 1, mp.mpf("0.3")),
        (mp.mpf("0.01"), 14, mp.mpf("-0.9")),
    ]:
        assert theta * phi * (1 + abs(rho)) < 4 and theta * phi**2 * (1 + abs(rho)) <= 4
        clean.append(
            [
                raw(theta * (1 - rho * rho) / 2),
                raw(theta * phi / 2),
                raw(rho),
                raw(-rho / phi),
                raw(mp.sqrt(1 - rho * rho) / phi),
            ]
        )
    clean.append([fx(4, 100), fx(1, 10), 0, 0, fx(2, 10)])
    clean.append([fx(1, 100), fx(5, 100), 0, fx(1, 10), fx(5, 100)])
    for i, p in enumerate(clean):
        assert real(p[1]) * (1 + abs(real(p[2]))) < 2
        gs = []
        for k in nodes(-3, 3, 601):
            r, g = row(f"clean{i}", p, k)
            assert g is not None and g > 0, (i, k, g)
            gs.append(g)
            out.append(r)
        print(f"clean{i}: {p}, min g {mp.nstr(min(gs), 6)}")

    near = 0
    while sum(1 for r in out if r[0] == "rand") < 6000:
        p = random_slice()
        if p is None:
            continue
        for _ in range(20):
            k = p[3] + raw(rng.uniform(-3, 3))
            r, g = row("rand", p, k)
            out.append(r)
            if g is not None and abs(g) < mp.mpf("1e-3"):
                near += 1
    print(f"rand: {sum(1 for r in out if r[0] == 'rand')} nodes, {near} with |g| < 1e-3")

    edges = 0
    while edges < 2000:
        p = random_slice()
        if p is None:
            continue
        coarse = [p[3] + raw(d / 10) for d in range(-30, 31)]
        gs = [w_and_g(p, k)[1] for k in coarse]
        for k0, k1, g0, g1 in zip(coarse, coarse[1:], gs, gs[1:]):
            if g0 is None or g1 is None or (g0 < 0) == (g1 < 0):
                continue
            lo, hi = mp.mpf(real(k0)), mp.mpf(real(k1))
            for _ in range(200):
                mid = (lo + hi) / 2
                gm = w_and_g(p, raw(mid))[1]
                if (gm < 0) == (g0 < 0):
                    lo = mid
                else:
                    hi = mid
            for _ in range(10):
                d = sign() * mp.mpf(10) ** rng.uniform(-19, -2)
                k = raw(lo + d)
                if abs(k) <= 32 * ONE:
                    out.append(row("edge", p, k)[0])
                    edges += 1
    print(f"edge: {edges} nodes within 10^-2 of a sign change of g")

    with open(sys.argv[1], "w") as f:
        for r in out:
            f.write(",".join(r) + "\n")


if __name__ == "__main__":
    main()
