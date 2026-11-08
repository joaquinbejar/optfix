// Replays every committed conformance generation (conformance/vectors-g*.json) through the
// JavaScript package that wasm-pack builds (`make wasm-test`), so the bindings and the wasm
// build of optfix are checked together: each case's result bit for bit, or its error code
// (docs/PLAN-improvements.md, milestone 17; the SVI functions from generation 4,
// docs/PLAN-phase2.md, milestone 25). Run with `node --test`.

import { test } from "node:test";
import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { readdirSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const require = createRequire(import.meta.url);
const wasm = require(process.env.OPTFIX_WASM_PKG ?? join(here, "..", "pkg", "optfix_wasm.js"));
const vectorsDir = process.env.OPTFIX_VECTORS_DIR ?? join(here, "..", "..", "conformance");

// Argument names that are not raw integers carried as bigint.
const STRING_ARGS = new Set(["kind", "rounding", "text"]);
const NUMBER_ARGS = new Set(["decimals", "shift", "nodes"]);
const SPECIAL = { from_decimal_str: "fixedFromDecimal", to_decimal: "fixedToDecimal" };

const camel = (f) => SPECIAL[f] ?? f.replace(/_([a-z0-9])/g, (_, c) => c.toUpperCase());

function convert(name, value) {
  if (STRING_ARGS.has(name)) return value;
  if (NUMBER_ARGS.has(name)) return Number(value);
  return BigInt(value);
}

// The generation 4 SVI functions (docs/PLAN-phase2.md, milestone 25): the first five
// arguments build an SviParams (whose constructor throws the case's DomainError), and each
// result flattens as the vectors write it: a boolean as "true" or "false", an enclosure as
// lo, hi, a grid report as ok, the first failure's k and bound if any, then the minimum's.
const slice = (args, at = 0) => new wasm.SviParams(...args.slice(at, at + 5).map(BigInt));
const pair = (o) => [o.lo, o.hi];
const report = (r) => [
  r.ok,
  ...(r.firstFailure === null ? [] : [r.firstFailure.k, r.firstFailure.bound]),
  r.min.k,
  r.min.bound,
];
const legs = (rest) => [
  rest.filter((_, i) => i % 2 === 0),
  rest.filter((_, i) => i % 2 === 1).map(BigInt),
];

const SVI = {
  svi_min_total_variance: (a) => [slice(a).minTotalVariance()],
  svi_total_variance: (a) => [slice(a).totalVariance(BigInt(a[5]))],
  svi_total_variance_bounds: (a) => pair(slice(a).totalVarianceBounds(BigInt(a[5]))),
  log_forward_moneyness: (a) => [wasm.logForwardMoneyness(...a.map(BigInt))],
  svi_vol: (a) => [wasm.sviVol(slice(a), ...a.slice(5).map(BigInt))],
  svi_vol_bounds: (a) => pair(wasm.sviVolBounds(slice(a), ...a.slice(5).map(BigInt))),
  svi_price_bounds: (a) =>
    pair(wasm.sviPriceBounds(slice(a), a[5], ...a.slice(6).map(BigInt))),
  price_strip_svi: (a) =>
    wasm.priceStripSvi(slice(a), ...a.slice(5, 9).map(BigInt), ...legs(a.slice(9))),
  check_wings: (a) => [wasm.checkWings(slice(a))],
  butterfly_g_bounds: (a) => pair(wasm.butterflyGBounds(slice(a), BigInt(a[5]))),
  check_butterfly: (a) =>
    report(wasm.checkButterfly(slice(a), BigInt(a[5]), BigInt(a[6]), Number(a[7]))),
  check_calendar: (a) =>
    report(
      wasm.checkCalendar(
        slice(a, 0),
        BigInt(a[5]),
        slice(a, 6),
        BigInt(a[11]),
        BigInt(a[12]),
        BigInt(a[13]),
        Number(a[14]),
      ),
    ),
};

function call(f, signature, args) {
  if (f in SVI) return SVI[f](args).map(String);
  const fn = wasm[camel(f)];
  assert.equal(typeof fn, "function", `no export for ${f} (${camel(f)})`);
  if (f === "price_strip" || f === "price_and_greeks_strip") {
    const market = args.slice(0, 5).map(BigInt);
    const legs = args.slice(5);
    const kinds = legs.filter((_, i) => i % 2 === 0);
    const strikes = legs.filter((_, i) => i % 2 === 1).map(BigInt);
    return fn(...market, kinds, strikes);
  }
  return fn(...args.map((a, i) => convert(signature[i], a)));
}

// A priced result's fields: five in generation 1, rho added from generation 2 (milestone 19).
const PRICED = ["price", "delta", "gamma", "vega", "theta", "rho"];
const pricedFields = (generation) => PRICED.slice(0, generation < 2 ? 5 : 6);

function flatten(out, fields) {
  if (Array.isArray(out)) return out.flatMap((o) => flatten(o, fields));
  if (typeof out === "object" && out !== null) return fields.map((k) => String(out[k]));
  return [String(out)];
}

// Milestone 20 refined implied_vol's guess and capped it at two steps: before generation 3 an
// implied_vol answer may move within the round-trip envelope, which the native replay checks
// (conformance/tests/replay.rs). Here such a case must keep its outcome: an answer, or the same
// error code. From generation 3 every case is bit for bit, implied_vol included.
const IV_EXACT_FROM = 3;
const ivMoved = (generation, c, got) =>
  c.f === "implied_vol" &&
  generation < IV_EXACT_FROM &&
  (c.e !== undefined ? got.e === c.e : got.r !== undefined);

const files = readdirSync(vectorsDir)
  .filter((n) => /^vectors-g\d+\.json$/.test(n))
  .sort((a, b) => Number(a.match(/\d+/)[0]) - Number(b.match(/\d+/)[0]));

test("there is a conformance generation", () => assert.ok(files.length > 0));

for (const file of files) {
  test(`${file} replays bit for bit through the wasm package`, () => {
    const v = JSON.parse(readFileSync(join(vectorsDir, file), "utf8"));
    const signatures = Object.fromEntries(v.signatures);
    const mismatches = [];
    for (const c of v.cases) {
      let got;
      try {
        got = { r: flatten(call(c.f, signatures[c.f], c.a), pricedFields(v.generation)) };
      } catch (e) {
        if (typeof e?.code !== "number") throw e;
        got = { e: e.code };
      }
      const exact = c.e !== undefined ? got.e === c.e : JSON.stringify(got.r) === JSON.stringify(c.r);
      const ok = exact || ivMoved(v.generation, c, got);
      if (!ok) mismatches.push(`${JSON.stringify(c)}\n  now: ${JSON.stringify(got)}`);
    }
    assert.equal(mismatches.length, 0, `${mismatches.length} of ${v.cases.length} differ:\n${mismatches.slice(0, 5).join("\n")}`);
  });
}
