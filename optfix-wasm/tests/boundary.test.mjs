// The JavaScript boundary of optfix-wasm (docs/PLAN-improvements.md, milestone 17): every
// numeric argument is converted exactly or rejected with DomainError (code 1), never wrapped or
// truncated as wasm-bindgen's own integer conversions would (`2n ** 127n` to `−2^127`, `2.7` to
// `2`). Run with `node --test` after `wasm-pack build` (`make wasm-test`).

import { test } from "node:test";
import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const require = createRequire(import.meta.url);
const of = require(process.env.OPTFIX_WASM_PKG ?? join(here, "..", "pkg", "optfix_wasm.js"));

const ONE = 1n << 64n;
const I128_MAX = (1n << 127n) - 1n;
const I128_MIN = -(1n << 127n);
const U128_MAX = (1n << 128n) - 1n;

function domainError(f, what) {
  assert.throws(f, (e) => e.code === 1 && e.name === "DomainError", what);
}

test("i128 arguments: the range ends pass, one past them is a DomainError", () => {
  assert.equal(of.mulDown(I128_MAX, ONE), I128_MAX);
  assert.equal(of.mulDown(I128_MIN, ONE), I128_MIN);
  domainError(() => of.mulDown(I128_MAX + 1n, ONE), "2^127");
  domainError(() => of.mulDown(I128_MIN - 1n, ONE), "−2^127 − 1");
  domainError(() => of.mulDown(1n << 200n, ONE), "2^200");
  domainError(() => of.fixedToDecimal(1n << 127n), "fixedToDecimal(2^127)");
  domainError(() => of.exp(5), "a number where a bigint is expected");
  domainError(() => of.exp("5"), "a string where a bigint is expected");
  domainError(() => of.exp(undefined), "undefined");
});

test("u128 arguments: negative and above 2^128 − 1 are DomainErrors", () => {
  assert.equal(of.mulShr(U128_MAX, 1n, 0, "down"), U128_MAX);
  domainError(() => of.mulShr(-1n, 1n, 0, "down"), "a = −1");
  domainError(() => of.mulShr(U128_MAX + 1n, 1n, 0, "down"), "a = 2^128");
});

test("small integer arguments: integers in range only", () => {
  assert.equal(of.fromScaled(1000n, 3, "down"), ONE);
  domainError(() => of.fromScaled(1000n, 2.7, "down"), "decimals 2.7");
  domainError(() => of.fromScaled(1000n, -1, "down"), "decimals −1");
  domainError(() => of.fromScaled(1000n, 256, "down"), "decimals 256");
  domainError(() => of.fromScaled(1000n, NaN, "down"), "decimals NaN");
  domainError(() => of.fromScaled(1000n, 3n, "down"), "decimals as a bigint");
  assert.equal(of.mulShr(ONE, ONE, 64, "down"), ONE);
  domainError(() => of.mulShr(ONE, ONE, 2 ** 32 + 64, "down"), "shift 2^32 + 64");
  domainError(() => of.mulShr(ONE, ONE, Infinity, "down"), "shift ∞");
});

test("i64 arguments: seconds beyond the i64 range are DomainErrors", () => {
  assert.equal(of.yearsFromSeconds(31_536_000n, "down"), ONE);
  domainError(() => of.yearsFromSeconds(1n << 70n, "down"), "2^70 seconds");
  domainError(() => of.yearsFromSeconds(31_536_000, "down"), "seconds as a number");
});

test("strings and strips", () => {
  domainError(() => of.fromRatio(1n, 3n, "sideways"), "rounding");
  domainError(() => of.price("straddle", ONE, ONE, ONE, ONE / 5n, 0n, 0n), "kind");
  const m = [ONE, 100n * ONE, ONE / 5n, 0n, 0n];
  assert.equal(of.priceStrip(...m, ["call"], [100n * ONE]).length, 1);
  domainError(() => of.priceStrip(...m, ["call"], [1n << 130n]), "strike 2^130");
  domainError(() => of.priceStrip(...m, ["call", "put"], [100n * ONE]), "lengths differ");
});

// docs/PLAN-phase2.md, milestone 25: the SVI surface.
test("SviParams: the constructor checks the domain and its arguments exactly", () => {
  const p = [ONE / 25n, (ONE * 2n) / 5n, -ONE / 2n, 0n, ONE / 10n];
  const s = new of.SviParams(...p);
  assert.deepEqual([s.a, s.b, s.rho, s.m, s.sigma], p);
  assert.ok(s.minTotalVariance() > 0n);
  domainError(() => new of.SviParams(p[0], -1n, p[2], p[3], p[4]), "b < 0");
  domainError(() => new of.SviParams(p[0], p[1], ONE, p[3], p[4]), "ρ = 1");
  domainError(() => new of.SviParams(-ONE, p[1], p[2], p[3], p[4]), "a negative minimum");
  domainError(() => new of.SviParams(p[0], p[1], p[2], p[3], 1n << 130n), "σ = 2^130");
  domainError(() => new of.SviParams(0.04, p[1], p[2], p[3], p[4]), "a as a number");
  domainError(() => s.totalVariance(33n * ONE), "|k| > 32");
  domainError(() => s.totalVariance(1), "k as a number");
});

test("SVI results are plain objects of bigints; grids take an integer node count", () => {
  const s = new of.SviParams(ONE / 25n, (ONE * 2n) / 5n, -ONE / 2n, 0n, ONE / 10n);
  const w = s.totalVarianceBounds(0n);
  assert.equal(Object.getPrototypeOf(w), Object.prototype);
  assert.deepEqual(Object.keys(w), ["lo", "hi"]);
  assert.ok(typeof w.lo === "bigint" && w.lo <= s.totalVariance(0n) && s.totalVariance(0n) <= w.hi);
  const r = of.checkButterfly(s, -ONE, ONE, 21);
  assert.deepEqual(Object.keys(r), ["ok", "firstFailure", "min"]);
  assert.equal(r.ok, true);
  assert.equal(r.firstFailure, null);
  assert.deepEqual(Object.keys(r.min), ["k", "bound"]);
  domainError(() => of.checkButterfly(s, -ONE, ONE, 21.5), "nodes 21.5");
  domainError(() => of.checkButterfly(s, -ONE, ONE, 21n), "nodes as a bigint");
  domainError(() => of.checkButterfly(s, -ONE, ONE, 0), "nodes 0");
  domainError(() => of.checkCalendar(s, ONE, s, ONE, -ONE, ONE, 3), "equal expiries");
  assert.throws(() => of.checkWings({ a: 0n }), "a plain object is not an SviParams");
  const m = [ONE, 100n * ONE, ONE / 25n, ONE / 100n];
  assert.equal(of.priceStripSvi(s, ...m, ["call", "put"], [100n * ONE, 90n * ONE]).length, 2);
  domainError(() => of.priceStripSvi(s, ...m, ["call"], [100n * ONE, 90n * ONE]), "lengths differ");
  domainError(() => of.priceStripSvi(s, 0n, ...m.slice(1), ["call"], [100n * ONE]), "T = 0");
});
