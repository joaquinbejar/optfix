"""Writes the committed fixture optfix/tests/data/iv_threshold.csv: targets whose exact
implied volatility has a vega just above or just below `implied_vol`'s conditioning threshold
`IV_VEGA_MIN_FACTOR × E = 2^28 × E` (`E = 2^-48 max(S, K) + 64 ulps`, rounded up as optfix
rounds it). Needs mpmath (a local dev venv; not a crate dependency).

For random inputs over the supported domain, σ is solved (on either side of vega's peak in σ)
so that vega(σ) = thr × (1 + ε) for ε in ±2^-6, ±2^-10, ±2^-14, ±2^-18, ±2^-22, ±2^-26 and 0;
the target is the exact price rounded to raw Q64.64, and the row then carries the exact
implied volatility of the rounded target and the exact vega there, in the format of
gen_iv_fixture.py: `S, K, T, σ, r, q, kind, target, sigma_star, vega_star` (raw).

    python gen_iv_threshold.py OUT.csv
"""
import random
import sys

import mpmath as m

sys.path.insert(0, __import__('os').path.dirname(__file__))
from gen_iv_fixture import S, SF, MIN_S, MAX_S, bsm, implied  # noqa: E402

m.mp.dps = 60
EPS = [0] + [sg * m.mpf(2) ** -k for k in (6, 10, 14, 18, 22, 26) for sg in (1, -1)]
V_MIN, V_MAX = int(m.mpf('0.005') * S) + 1, 5 * S


def threshold(s_raw, k_raw):
    e_raw = -((-(1 << 16) * max(s_raw, k_raw)) >> 64) + 64   # mul_up, then + 64 ulps
    return m.mpf(e_raw << 28) / SF


def solve_sigma(put, s, k, t, r, q, want, lo, hi, rising):
    for _ in range(200):
        mid = (lo + hi) / 2
        v = bsm(put, s, k, t, mid, r, q)[1]
        if (v < want) == rising:
            lo = mid
        else:
            hi = mid
    return (lo + hi) / 2


def main():
    rng = random.Random(0x7e5)
    rows = []
    while len(rows) < 5000:
        s_raw = int(m.mpf(10) ** rng.uniform(-6, 9) * S)
        k_raw = int(s_raw * m.mpf(10) ** rng.uniform(-1.3, 1.3))
        if not (MIN_S <= s_raw <= MAX_S and MIN_S <= k_raw <= MAX_S
                and s_raw * 20 >= k_raw and k_raw * 20 >= s_raw):
            continue
        t_raw = int(m.mpf(10) ** rng.uniform(-8, m.log10(30)) * S)
        r_raw, q_raw = int(rng.uniform(-.1, .5) * S), int(rng.uniform(-.1, .5) * S)
        put = rng.randrange(2)
        s, k, t, r, q = (m.mpf(x) / SF for x in (s_raw, k_raw, t_raw, r_raw, q_raw))
        thr = threshold(s_raw, k_raw)
        lo_s, hi_s = m.mpf(V_MIN) / SF, m.mpf(V_MAX) / SF
        # vega's peak in σ (vega rises, then falls): golden-section on ln σ
        a, b = m.log(lo_s), m.log(hi_s)
        for _ in range(120):
            c1, c2 = a + (b - a) * 0.382, a + (b - a) * 0.618
            if bsm(put, s, k, t, m.exp(c1), r, q)[1] < bsm(put, s, k, t, m.exp(c2), r, q)[1]:
                a = c1
            else:
                b = c2
        peak = m.exp((a + b) / 2)
        v_peak = bsm(put, s, k, t, peak, r, q)[1]
        branches = [(lo_s, peak, True), (peak, hi_s, False)]
        for eps in EPS:
            want = thr * (1 + eps)
            if not want < v_peak:
                break
            lo, hi, rising = branches[rng.randrange(2)]
            end = bsm(put, s, k, t, lo if rising else hi, r, q)[1]
            if not end < want:
                continue
            sig = solve_sigma(put, s, k, t, r, q, want, lo, hi, rising)
            v_raw = int(m.nint(sig * SF))
            if not V_MIN <= v_raw <= V_MAX:
                continue
            sig = m.mpf(v_raw) / SF
            target = int(m.nint(bsm(put, s, k, t, sig, r, q)[0] * SF))
            star = implied(put, s, k, t, r, q, m.mpf(target) / SF, sig) if target > 0 else None
            if star is None:
                continue
            vstar = bsm(put, s, k, t, star, r, q)[1]
            rows.append((s_raw, k_raw, t_raw, v_raw, r_raw, q_raw, put, target,
                         int(m.nint(star * SF)), int(m.nint(vstar * SF))))
    with open(sys.argv[1], 'w') as out:
        out.write(''.join(','.join(map(str, x)) + '\n' for x in rows))
    print(len(rows))


if __name__ == '__main__':
    main()
