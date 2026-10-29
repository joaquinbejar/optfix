"""Writes the committed fixture optfix/tests/data/iv_fixture.csv: an independent mpmath
implied-volatility reference on a boundary set in the style of codex's #11 sweep
(tools/bsm_fixture/gen_sweep.py): d1 in {−9, −5, −1, −1/4, 0, 1/4, 1, 5, 9} at 26 expiries from
1e-12 s to one year, five volatilities, four spot scales and three rate/carry sets (K solved
from d1), calls and puts. Needs mpmath (a local dev venv; not a crate dependency).

Each row is the inputs `S, K, T, σ, r, q, kind` (raw Q64.64, kind 0 call, 1 put), then

- `target`: the exact Black-Scholes-Merton price at `σ` (mpmath, 60 digits), rounded to the
  nearest raw Q64.64 value;
- `sigma_star`: the exact implied volatility of that rounded target (Newton in `ln σ` from
  `σ`, to 40 digits), rounded to raw, or −1 if the root is outside `[10^-8, 10^3]` or the
  target is not strictly inside its no-arbitrage bounds;
- `vega_star`: the exact vega `S Dq n(d1) √T` at `sigma_star`, rounded to raw (−1 with it).

    python gen_iv_fixture.py OUT.csv
"""
import sys

import mpmath as m

m.mp.dps = 60
S = 1 << 64
SF = m.mpf(S)
MIN_S, MAX_S = 18446744073709, 10**9 * S
SECS = ['1e-12', '1e-10', '1e-8', '1e-6', '1e-4', '.001', '.01', '.03', '.1', '.3', '1', '2',
        '3', '5', '10', '20', '30', '60', '120', '300', '600', '1800', '3600', '86400',
        '2592000', '31536000']
D1S = ['-9', '-5', '-1', '-.25', '0', '.25', '1', '5', '9']


def N(z):
    return m.erfc(-z / m.sqrt(2)) / 2


def bsm(put, s, k, t, sig, r, q):
    """(price, vega)"""
    dq, dr, a = m.exp(-q * t), m.exp(-r * t), sig * m.sqrt(t)
    d1 = (m.log(s / k) + (r - q + sig * sig / 2) * t) / a
    d2 = d1 - a
    vega = s * dq * m.exp(-d1 * d1 / 2) / m.sqrt(2 * m.pi) * m.sqrt(t)
    if put:
        return k * dr * N(-d2) - s * dq * N(-d1), vega
    return s * dq * N(d1) - k * dr * N(d2), vega


def implied(put, s, k, t, r, q, target, sig0):
    dq, dr = m.exp(-q * t), m.exp(-r * t)
    lower = max(0, (k * dr - s * dq) if put else (s * dq - k * dr))
    upper = k * dr if put else s * dq
    if not lower < target < upper:
        return None
    v = m.log(sig0)
    for _ in range(100):
        sig = m.exp(v)
        p, vega = bsm(put, s, k, t, sig, r, q)
        if vega == 0:
            return None
        step = (p - target) / (vega * sig)
        v -= step
        if not m.log(m.mpf('1e-8')) < v < m.log(1000):
            return None
        if abs(step) < m.mpf(10) ** -40:
            return m.exp(v)
    return None


def main():
    rows = []
    for sec in SECS:
        t_raw = max(1, int(m.mpf(sec) / 31536000 * S))
        t = m.mpf(t_raw) / SF
        for sig_s in ['.005', '.2', '.5', '1', '5']:
            v_raw = int(m.mpf(sig_s) * S)
            sig = m.mpf(v_raw) / SF
            for s_raw in [MIN_S, S, 100 * S, MAX_S]:
                s = m.mpf(s_raw) / SF
                for rr, qq in [('0', '0'), ('.05', '.02'), ('-.1', '.5')]:
                    r_raw = int(m.floor(m.mpf(rr) * S))
                    q_raw = int(m.floor(m.mpf(qq) * S))
                    r, q = m.mpf(r_raw) / SF, m.mpf(q_raw) / SF
                    for d1 in D1S:
                        k = s * m.exp((r - q + sig * sig / 2) * t - m.mpf(d1) * sig * m.sqrt(t))
                        k_raw = int(m.nint(k * SF))
                        if not (MIN_S <= k_raw <= MAX_S and s_raw * 20 >= k_raw
                                and k_raw * 20 >= s_raw):
                            continue
                        k = m.mpf(k_raw) / SF
                        for put in [0, 1]:
                            p, _ = bsm(put, s, k, t, sig, r, q)
                            target = int(m.nint(p * SF))
                            star = implied(put, s, k, t, r, q, m.mpf(target) / SF, sig) \
                                if target > 0 else None
                            if star is None:
                                sstar = vstar = -1
                            else:
                                sstar = int(m.nint(star * SF))
                                vstar = int(m.nint(bsm(put, s, k, t, star, r, q)[1] * SF))
                            rows.append((s_raw, k_raw, t_raw, v_raw, r_raw, q_raw, put, target,
                                         sstar, vstar))
    with open(sys.argv[1], 'w') as out:
        out.write(''.join(','.join(map(str, x)) + '\n' for x in rows))
    print(len(rows))


if __name__ == '__main__':
    main()
