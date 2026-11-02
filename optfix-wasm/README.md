# optfix-wasm

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

The repository's `conformance/vectors-g*.json` list about 20,000 calls each with their exact
results; CI replays every generation through this package on Node.js.

License: MIT.
