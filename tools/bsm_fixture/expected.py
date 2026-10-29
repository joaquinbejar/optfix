"""Writes the committed fixture optfix/tests/data/bsm_fixture.csv: each input row of
gen_sweep.py followed by the exact Black-Scholes-Merton price, delta, gamma, vega and theta,
evaluated with mpmath at 100 digits (as in check_mpmath.py) and rounded to the nearest raw
Q64.64 value. Needs mpmath (a local dev venv; not a crate dependency).

    python expected.py INPUTS.csv OUT.csv
"""
import csv
import sys

import mpmath as mp

mp.mp.dps = 100
S = mp.mpf(2) ** 64


def N(z):
    return mp.erfc(-z / mp.sqrt(2)) / 2


def phi(z):
    return mp.exp(-z * z / 2) / mp.sqrt(2 * mp.pi)


def main():
    out = []
    for row in csv.reader(open(sys.argv[1])):
        raw = list(map(int, row[:7]))
        s, k, t, sig, r, q = [mp.mpf(v) / S for v in raw[:6]]
        put = raw[6]
        dq, dr, rt = mp.exp(-q * t), mp.exp(-r * t), mp.sqrt(t)
        a = sig * rt
        d1 = (mp.log(s / k) + (r - q + sig * sig / 2) * t) / a
        d2 = d1 - a
        pdf = phi(d1)
        if put:
            price = k * dr * N(-d2) - s * dq * N(-d1)
            delta = -dq * N(-d1)
            theta = -s * dq * pdf * sig / (2 * rt) + r * k * dr * N(-d2) - q * s * dq * N(-d1)
        else:
            price = s * dq * N(d1) - k * dr * N(d2)
            delta = dq * N(d1)
            theta = -s * dq * pdf * sig / (2 * rt) - r * k * dr * N(d2) + q * s * dq * N(d1)
        wants = [price, delta, dq * pdf / (s * a), s * dq * pdf * rt, theta]
        out.append(",".join(map(str, raw + [int(mp.nint(w * S)) for w in wants])))
    open(sys.argv[2], "w").write("\n".join(out) + "\n")
    print(len(out))


if __name__ == "__main__":
    main()
