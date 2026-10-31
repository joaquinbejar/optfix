//! Compute-unit benchmarks for optfix.
//!
//! The harness lives in `tests/cu.rs`: it loads the SBF build of `optfix-bench-program` into
//! LiteSVM, runs every instruction on deterministic inputs, checks each on-chain result against
//! the host result bit for bit, and prints the CU table (`make bench` writes it to
//! `docs/benchmarks.md`). Build the program first with `make program`.
//!
//! This library holds the CU budget check (`docs/PLAN.md`, milestone 8): [`check`] compares
//! the measured rows with the committed limits of `bench/budget.md`.

use std::collections::BTreeMap;
use std::fmt;

/// One measured bench row: net CU (gross for `Noop`), as the budget states them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Measured {
    /// Row name, as in `docs/benchmarks.md` without the backticks.
    pub name: String,
    /// Median CU.
    pub median: u64,
    /// Maximum CU.
    pub max: u64,
}

/// Why the budget check failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Violation {
    /// A table line of the budget that is not `` | `name` | median | max | ``.
    Malformed {
        /// 1-based line number in the budget file.
        line: usize,
        /// The line itself.
        text: String,
    },
    /// The same row twice in the budget.
    Duplicate(String),
    /// A measured row with no budget row.
    Unbudgeted(String),
    /// A budget row that is no longer measured.
    Unmeasured(String),
    /// The measured median is over its limit.
    MedianOver {
        /// Row name.
        name: String,
        /// Measured median.
        measured: u64,
        /// Committed limit.
        limit: u64,
    },
    /// The measured maximum is over its limit.
    MaxOver {
        /// Row name.
        name: String,
        /// Measured maximum.
        measured: u64,
        /// Committed limit.
        limit: u64,
    },
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed { line, text } => write!(f, "line {line} is malformed: {text}"),
            Self::Duplicate(n) => write!(f, "`{n}` appears twice"),
            Self::Unbudgeted(n) => write!(f, "`{n}` is measured but has no budget row"),
            Self::Unmeasured(n) => write!(f, "`{n}` has a budget row but is not measured"),
            Self::MedianOver {
                name,
                measured,
                limit,
            } => write!(f, "`{name}` median {measured} is over its limit {limit}"),
            Self::MaxOver {
                name,
                measured,
                limit,
            } => write!(f, "`{name}` max {measured} is over its limit {limit}"),
        }
    }
}

/// Result of [`check`]: the violations, and a Markdown table of every measured row against
/// its limits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// Empty when the budget holds.
    pub violations: Vec<Violation>,
    /// Measured value, limit and delta per row; rows more than 20% under a limit that the rule
    /// would tighten are marked \"loose\".
    pub table: String,
}

/// Committed limits of one row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Limit {
    median: u64,
    max: u64,
}

/// A row more than this fraction (in percent) under a limit is reported as loose.
const LOOSE_PERCENT: u64 = 20;

/// Parses the budget's table rows. Lines that do not start with `|` are prose; the header
/// and separator rows are skipped; every other `|` line must be a row.
fn parse(text: &str) -> (BTreeMap<String, Limit>, Vec<Violation>) {
    let mut limits = BTreeMap::new();
    let mut violations = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let t = line.trim();
        if !t.starts_with('|') || t.starts_with("| Row ") || t.starts_with("| ---") {
            continue;
        }
        let number = i.saturating_add(1);
        match parse_row(t) {
            Some((name, limit)) => {
                if limits.insert(name.clone(), limit).is_some() {
                    violations.push(Violation::Duplicate(name));
                }
            }
            None => violations.push(Violation::Malformed {
                line: number,
                text: line.to_string(),
            }),
        }
    }
    (limits, violations)
}

/// `` | `name` | median | max | `` with nothing after the closing bar.
fn parse_row(t: &str) -> Option<(String, Limit)> {
    let inner = t.strip_prefix('|')?.strip_suffix('|')?;
    let mut cells = inner.split('|').map(str::trim);
    let name = cells.next()?.strip_prefix('`')?.strip_suffix('`')?;
    let median = cells.next()?.parse().ok()?;
    let max = cells.next()?.parse().ok()?;
    if name.is_empty() || name.contains('`') || cells.next().is_some() {
        return None;
    }
    Some((name.to_string(), Limit { median, max }))
}

/// `measured` against `limit`: the signed delta and its per mille of the limit.
fn delta(measured: u64, limit: u64) -> String {
    let d = i128::from(measured).saturating_sub(i128::from(limit));
    let permille = d
        .saturating_mul(1000)
        .checked_div(i128::from(limit))
        .unwrap_or(0);
    let sign = if permille < 0 { "−" } else { "+" };
    let a = permille.unsigned_abs();
    let (whole, tenth) = (
        a.checked_div(10).unwrap_or(0),
        a.checked_rem(10).unwrap_or(0),
    );
    let ds = if d < 0 { "−" } else { "+" };
    format!("{ds}{} ({sign}{whole}.{tenth}%)", d.unsigned_abs())
}

/// The limit the budget rule gives a measurement: plus 5%, rounded up to a multiple of 100.
#[must_use]
pub fn rule_limit(measured: u64) -> u64 {
    let hundredths = u128::from(measured).saturating_mul(105);
    let hundreds = hundredths.div_ceil(10_000);
    u64::try_from(hundreds.saturating_mul(100)).unwrap_or(u64::MAX)
}

/// Whether `measured` is more than [`LOOSE_PERCENT`] under `limit` and the rule would give a
/// lower limit. Without the second condition the rounding to 100 would mark small rows (such
/// as `Exp`, 316 against 400) loose for ever.
fn loose(measured: u64, limit: u64) -> bool {
    let floor = u128::from(limit).saturating_mul(u128::from(100u64.saturating_sub(LOOSE_PERCENT)));
    u128::from(measured).saturating_mul(100) < floor && rule_limit(measured) < limit
}

/// Checks `measured` against the budget file's `text`.
#[must_use]
pub fn check(text: &str, measured: &[Measured]) -> Report {
    let (limits, mut violations) = parse(text);
    let mut table = String::from(
        "| Row | Median | Limit | Delta | Max | Limit | Delta |\n\
         | --- | ---: | ---: | ---: | ---: | ---: | ---: |\n",
    );
    for m in measured {
        let Some(l) = limits.get(&m.name) else {
            violations.push(Violation::Unbudgeted(m.name.clone()));
            continue;
        };
        if m.median > l.median {
            violations.push(Violation::MedianOver {
                name: m.name.clone(),
                measured: m.median,
                limit: l.median,
            });
        }
        if m.max > l.max {
            violations.push(Violation::MaxOver {
                name: m.name.clone(),
                measured: m.max,
                limit: l.max,
            });
        }
        let mark = |v: u64, lim: u64| if loose(v, lim) { " (loose)" } else { "" };
        table.push_str(&format!(
            "| `{}` | {} | {} | {}{} | {} | {} | {}{} |\n",
            m.name,
            m.median,
            l.median,
            delta(m.median, l.median),
            mark(m.median, l.median),
            m.max,
            l.max,
            delta(m.max, l.max),
            mark(m.max, l.max),
        ));
    }
    for name in limits.keys() {
        if !measured.iter().any(|m| &m.name == name) {
            violations.push(Violation::Unmeasured(name.clone()));
        }
    }
    Report { violations, table }
}

#[cfg(test)]
mod tests {
    //! Failing controls: each kind of violation, on its own, against the committed budget.
    #![allow(
        clippy::unwrap_used,
        clippy::indexing_slicing,
        clippy::panic,
        clippy::disallowed_macros,
        clippy::arithmetic_side_effects
    )]

    use super::*;

    const BUDGET: &str = include_str!("../budget.md");

    /// Every committed row measured exactly at its limits.
    fn at_limits() -> Vec<Measured> {
        let (limits, v) = parse(BUDGET);
        assert!(v.is_empty(), "committed budget: {v:?}");
        limits
            .into_iter()
            .map(|(name, l)| Measured {
                name,
                median: l.median,
                max: l.max,
            })
            .collect()
    }

    fn only(text: &str, measured: &[Measured]) -> Violation {
        let r = check(text, measured);
        assert_eq!(r.violations.len(), 1, "{:?}", r.violations);
        r.violations[0].clone()
    }

    #[test]
    fn test_committed_budget_holds_at_its_limits() {
        let m = at_limits();
        assert!(m.len() >= 18, "{} rows", m.len());
        assert!(check(BUDGET, &m).violations.is_empty());
    }

    #[test]
    fn test_median_one_over_fails() {
        let mut m = at_limits();
        m[3].median += 1;
        assert!(matches!(only(BUDGET, &m), Violation::MedianOver { .. }));
    }

    #[test]
    fn test_max_one_over_fails() {
        let mut m = at_limits();
        m[5].max += 1;
        assert!(matches!(only(BUDGET, &m), Violation::MaxOver { .. }));
    }

    #[test]
    fn test_missing_budget_row_fails() {
        let mut m = at_limits();
        m.push(Measured {
            name: "NewRow".into(),
            median: 1,
            max: 1,
        });
        assert_eq!(only(BUDGET, &m), Violation::Unbudgeted("NewRow".into()));
    }

    #[test]
    fn test_extra_budget_row_fails() {
        let mut m = at_limits();
        let gone = m.remove(2);
        assert_eq!(only(BUDGET, &m), Violation::Unmeasured(gone.name));
    }

    #[test]
    fn test_duplicate_budget_row_fails() {
        let m = at_limits();
        let text = format!("{BUDGET}| `{}` | 1000000 | 1000000 |\n", m[0].name);
        assert_eq!(only(&text, &m), Violation::Duplicate(m[0].name.clone()));
    }

    #[test]
    fn test_malformed_budget_rows_fail() {
        let m = at_limits();
        for bad in [
            "| `Exp` | 400 |",
            "| `Exp` | 400 | 400 | 1 |",
            "| Exp | 400 | 400 |",
            "| `Exp` | 4e2 | 400 |",
            "| `Exp` | -1 | 400 |",
            "| `` | 400 | 400 |",
        ] {
            let text = format!("{BUDGET}{bad}\n");
            assert!(
                matches!(only(&text, &m), Violation::Malformed { .. }),
                "{bad}"
            );
        }
    }

    #[test]
    fn test_rule_limit() {
        assert_eq!(rule_limit(0), 0);
        assert_eq!(rule_limit(95), 100);
        assert_eq!(rule_limit(96), 200);
        assert_eq!(rule_limit(316), 400);
        assert_eq!(rule_limit(14_641), 15_400);
        assert_eq!(rule_limit(14_666), 15_400);
        assert_eq!(rule_limit(14_667), 15_500);
    }

    #[test]
    fn test_committed_limits_follow_the_rule_on_0_1_0() {
        // The 0.1.0 measurement (docs/benchmarks.md at v0.1.0) the initial limits came from.
        for (name, median, max) in [("PriceAndGreeks", 14_240, 14_641), ("Exp", 316, 337)] {
            let (limits, _) = parse(BUDGET);
            let l = limits[name];
            assert_eq!(
                (l.median, l.max),
                (rule_limit(median), rule_limit(max)),
                "{name}"
            );
        }
    }

    #[test]
    fn test_rounding_alone_is_not_loose() {
        let m = [Measured {
            name: "Exp".into(),
            median: 316,
            max: 337,
        }];
        let r = check("| `Exp` | 400 | 400 |\n", &m);
        assert!(!r.table.contains("loose"), "{}", r.table);
    }

    #[test]
    fn test_report_prints_deltas_and_loose_rows() {
        let text = "| `A` | 1000 | 1000 |\n";
        let m = [Measured {
            name: "A".into(),
            median: 799,
            max: 1000,
        }];
        let r = check(text, &m);
        assert!(r.violations.is_empty());
        assert!(
            r.table.contains(
                "| `A` | 799 | 1000 | −201 (−20.1%) (loose) | 1000 | 1000 | +0 (+0.0%) |"
            ),
            "{}",
            r.table
        );
    }
}

#[cfg(test)]
mod scratch_ci_control {
    #[test]
    #[allow(clippy::panic)]
    fn fails() {
        panic!("scratch CI control");
    }
}
