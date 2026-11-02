// Replays every committed conformance generation (conformance/vectors-g*.json) through the
// JavaScript package that wasm-pack builds (`make wasm-test`), so the bindings and the wasm
// build of optfix are checked together: each case's result bit for bit, or its error code
// (docs/PLAN-improvements.md, milestone 17). Run with `node --test`.

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
const NUMBER_ARGS = new Set(["decimals", "shift"]);
const SPECIAL = { from_decimal_str: "fixedFromDecimal", to_decimal: "fixedToDecimal" };

const camel = (f) => SPECIAL[f] ?? f.replace(/_([a-z0-9])/g, (_, c) => c.toUpperCase());

function convert(name, value) {
  if (STRING_ARGS.has(name)) return value;
  if (NUMBER_ARGS.has(name)) return Number(value);
  return BigInt(value);
}

function call(f, signature, args) {
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
      const ok = c.e !== undefined ? got.e === c.e : JSON.stringify(got.r) === JSON.stringify(c.r);
      if (!ok) mismatches.push(`${JSON.stringify(c)}\n  now: ${JSON.stringify(got)}`);
    }
    assert.equal(mismatches.length, 0, `${mismatches.length} of ${v.cases.length} differ:\n${mismatches.slice(0, 5).join("\n")}`);
  });
}
