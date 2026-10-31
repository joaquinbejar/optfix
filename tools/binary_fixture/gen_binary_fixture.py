"""Binary-price fixture for milestone 13 (docs/PLAN.md section 8): exact cash-or-nothing and
asset-or-nothing prices at 100 digits (mpmath) for the Q64.64 inputs as given, on the
boundary sets the plan asks for: d2 (cash) or d1 (asset) in {−9, −5, −1, −1/4, 0, 1/4, 1, 5, 9}
at 26 expiries from 1e-12 s to 30 years, four volatilities, four spot scales and three
rate/carry sets (the strike solved from the target d), plus strikes at the money, on both
sides of the d1 switch (|S − K| × 256 = K) and at T = 0. Needs mpmath (a local dev venv; not
a crate dependency).

    python gen_binary_fixture.py > optfix/tests/data/binary_fixture.csv

Columns: kind (0 call, 1 put), S, K, T, sigma, r, q, payout (raw Q64.64), then the exact
cash-or-nothing and asset-or-nothing prices as raw Q64.64 rounded to nearest.
"""
import random
import sys

import mpmath as m

m.mp.dps = 100
ONE = 1 << 64
MIN_S, MAX_S = 18446744073709, 10**9 * ONE
rng = random.Random(1300)
SECS = ['1e-12', '1e-10', '1e-8', '1e-6', '1e-4', '.001', '.01', '.03', '.1', '.3', '1', '2',
        '3', '5', '10', '20', '30', '60', '120', '300', '600', '1800', '3600', '86400',
        '2592000', '946080000']


def exact(kind, s, k, t, sig, r, q, pay):
    """Exact prices for raw inputs; T = 0 pays strictly in the money."""
    S, K, T, V, R, Q, P = (m.mpf(x) / ONE for x in (s, k, t, sig, r, q, pay))
    if t == 0:
        itm = S > K if kind == 0 else S < K
        return (pay, s) if itm else (0, 0)
    sst = V * m.sqrt(T)
    d1 = (m.log(S / K) + (R - Q + V * V / 2) * T) / sst
    d2 = d1 - sst
    sign = 1 if kind == 0 else -1
    cash = P * m.exp(-R * T) * m.ncdf(sign * d2)
    asset = S * m.exp(-Q * T) * m.ncdf(sign * d1)
    return int(m.nint(cash * ONE)), int(m.nint(asset * ONE))


def valid(s, k):
    return MIN_S <= k <= MAX_S and s * 20 >= k and k * 20 >= s


rows = []
for sec in SECS:
    t = max(1, int(m.mpf(sec) / 31536000 * ONE))
    T = m.mpf(t) / ONE
    for sig in ['.005', '.2', '1', '5']:
        vr = int(m.mpf(sig) * ONE)
        V = m.mpf(vr) / ONE
        for sr in [MIN_S, ONE, 100 * ONE, MAX_S]:
            S = m.mpf(sr) / ONE
            for rr, qq in [('0', '0'), ('.05', '.02'), ('-.1', '.5')]:
                rraw = int(m.floor(m.mpf(rr) * ONE))
                qraw = int(m.floor(m.mpf(qq) * ONE))
                R, Q = m.mpf(rraw) / ONE, m.mpf(qraw) / ONE
                for which in ('d2', 'd1'):
                    for d in [-9, -5, -1, '-.25', 0, '.25', 1, 5, 9]:
                        d = m.mpf(d)
                        # Solve K for the target d1 (or d2 = d1 − σ√T).
                        d1 = d + V * m.sqrt(T) if which == 'd2' else d
                        k = S * m.exp((R - Q + V * V / 2) * T - d1 * V * m.sqrt(T))
                        kr = int(m.nint(k * ONE))
                        if valid(sr, kr):
                            pay = int(m.mpf(10) ** rng.uniform(-6, 9) * ONE)
                            for kind in (0, 1):
                                rows.append((kind, sr, kr, t, vr, rraw, qraw, pay))
# At the money, both sides of the d1 switch, and T = 0, on random inputs.
for _ in range(3000):
    sr = int(m.mpf(10) ** rng.uniform(-6, 9) * ONE)
    sr = min(max(sr, MIN_S), MAX_S)
    choice = rng.randrange(4)
    if choice == 0:
        kr = sr + rng.randrange(-64, 65)
    elif choice == 1:
        kr = sr // 255 * 256 + rng.randrange(-4, 5)
    elif choice == 2:
        kr = sr // 257 * 256 + rng.randrange(-4, 5)
    else:
        kr = int(sr * m.mpf(10) ** rng.uniform(-1.3, 1.3))
    if not valid(sr, kr):
        continue
    t = 0 if rng.randrange(5) == 0 else int(m.mpf(10) ** rng.uniform(-12, m.log10(30)) * ONE)
    vr = int(m.mpf(10) ** rng.uniform(m.log10(m.mpf('.005')), m.log10(5)) * ONE)
    rraw = int(rng.uniform(-.1, .5) * ONE)
    qraw = int(rng.uniform(-.1, .5) * ONE)
    pay = int(m.mpf(10) ** rng.uniform(-6, 9) * ONE)
    rows.append((rng.randrange(2), sr, kr, t, vr, rraw, qraw, pay))

out = sys.stdout
for row in rows:
    cash, asset = exact(*row)
    out.write(','.join(map(str, row + (cash, asset))) + '\n')
print(len(rows), file=sys.stderr)
