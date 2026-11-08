# optfix-wasm

[![npm](https://img.shields.io/npm/v/optfix-wasm.svg)](https://www.npmjs.com/package/optfix-wasm)
[![Crates.io](https://img.shields.io/crates/v/optfix-wasm.svg)](https://crates.io/crates/optfix-wasm)

WebAssembly bindings for [optfix](https://github.com/joaquinbejar/optfix): fixed-point
(Q64.64) option pricing with every rounding explicit, bit-identical in JavaScript to the Rust
crate and to its on-chain (Solana SBF) build.

Every value crosses the boundary exactly. A Q64.64 number is its raw `bigint` (`raw / 2^64`);
`fixedFromDecimal` and `fixedToDecimal` convert from and to exact decimal text. Nothing is a
JavaScript `number`. An option kind is `"call"` or `"put"`, a rounding `"down"` (toward
negative infinity) or `"up"`. A failure throws an `OptfixError` with `code` (the Rust
`PricingError` discriminant: 0 `Overflow`, 1 `DomainError`, 2 `NoConvergence`,
3 `ArbitrageBound`, 4 `IllConditioned`) and `name`.

```shell
npm install optfix-wasm
```

```js
const of = require("optfix-wasm");
const x = (s) => of.fixedFromDecimal(s, "down");

const premium = of.price("call", x("100"), x("105"), x("0.5"), x("0.3"), x("0.04"), x("0"));
console.log(of.fixedToDecimal(premium)); // the exact value of the Q64.64 result

const g = of.priceAndGreeks("put", x("100"), x("95"), x("1"), x("0.2"), x("0.03"), x("0.01"));
console.log(of.fixedToDecimal(g.delta), of.fixedToDecimal(g.rho)); // also gamma, vega, theta

try {
  of.price("call", x("100"), x("10000"), x("1"), x("0.2"), x("0"), x("0")); // S/K below 0.05
} catch (e) {
  console.log(e.code, e.name); // 1 DomainError
}
```

Functions: `price`, `priceAndGreeks`, `priceBlack76`, `priceAndGreeksBlack76`,
`priceCashOrNothing`, `priceAssetOrNothing`, `priceStrip`, `priceAndGreeksStrip`, `impliedVol`,
`exp`, `ln`, `normCdf`, `normPdf`, `mulDown`, `mulUp`, `divDown`, `divUp`, `sqrtDown`,
`sqrtUp`, `fromRatio`, `fromBps`, `fromScaled`, `toScaled`, `mulShr`, `yearsFromSeconds`,
`fixedFromDecimal`, `fixedToDecimal`. Their contracts and error bounds are optfix's
([docs.rs/optfix](https://docs.rs/optfix)).

## SVI volatility slices

A raw SVI slice is an `SviParams` object, `new SviParams(a, b, rho, m, sigma)` with raw
`bigint`s, whose constructor throws `DomainError` outside optfix's domain (including a slice
whose minimum total variance may be negative). An enclosure comes back as a plain object
`{ lo, hi }` and a grid check as `{ ok, firstFailure, min }`, each node `{ k, bound }`
(`firstFailure` is `null` when every node passes).

```js
const of = require("optfix-wasm");
const x = (s) => of.fixedFromDecimal(s, "down");

// A raw SVI slice a, b, ρ, m, σ (total variance against k = ln(K / F)).
const slice = new of.SviParams(x("0.005"), x("0.1"), x("-0.4"), x("0"), x("0.1"));
const [T, S, r, q] = [x("0.25"), x("100"), x("0.04"), x("0.01")];

const vol = of.sviVol(slice, T, S, r, q, x("90")); // the slice's volatility at K = 90
console.log(of.fixedToDecimal(vol)); // 0.3136...

// One premium per leg, each equal bit for bit to price(...) at sviVol(...) for its strike.
const prices = of.priceStripSvi(slice, T, S, r, q, ["put", "call", "call"], [x("90"), x("100"), x("110")]);
console.log(prices.map(of.fixedToDecimal)); // [ '2.0393...', '5.2869...', '1.7433...' ]

// Encloses the premium at the slice's exact volatility.
const { lo, hi } = of.sviPriceBounds(slice, "put", S, x("90"), T, r, q);
console.log(of.fixedToDecimal(lo), of.fixedToDecimal(hi));

console.log(of.checkWings(slice)); // true: b (1 + |ρ|) < 2, certain
const report = of.checkButterfly(slice, x("-2"), x("2"), 81); // Durrleman's g at 81 nodes
console.log(report.ok, report.firstFailure); // true null

try {
  new of.SviParams(x("-0.1"), x("0.4"), x("-0.4"), x("0"), x("0.1")); // minimum below 0
} catch (e) {
  console.log(e.code, e.name); // 1 DomainError
}
```

`SviParams` has the getters `a`, `b`, `rho`, `m`, `sigma` and the methods
`minTotalVariance()`, `totalVariance(k)` and `totalVarianceBounds(k)`. The functions are
`logForwardMoneyness(expiry, spot, rate, carry, strike)`, `sviVol` and `sviVolBounds`
`(slice, expiry, spot, rate, carry, strike)`, `sviPriceBounds(slice, kind, spot, strike,
expiry, rate, carry)`, `priceStripSvi(slice, expiry, spot, rate, carry, kinds, strikes)`,
`checkWings(slice)`, `butterflyGBounds(slice, k)`, `checkButterfly(slice, kMin, kMax, nodes)`
and `checkCalendar(earlier, expiryEarlier, later, expiryLater, kMin, kMax, nodes)`, with
`nodes` a JavaScript `number`. A grid check is conservative at its nodes and says nothing
between them; a failing strip leg throws its error. The contracts and error bounds are
optfix's.

## Conformance

The repository's `conformance/vectors-g*.json` list about 20,000 calls each (31,000 from
generation 4, which adds the SVI functions) with their exact results; CI replays every
generation through this package on Node.js.

License: MIT.
