# Codex's #12 threshold set

Codex's independent adversarial set for `implied_vol`'s conditioning threshold (#12 review,
bus messages #37114 to #37124), computed at 100 mpmath digits with no optfix code:

- `gen_conditioning.py` writes the 10,005 inputs (five `(σ, T)` pairs, strikes stepped by
  `10^-9` in `ln K` around the threshold);
- `export_exact_threshold.py` adds each exact root, its vega and the exact ratio of that vega
  to `2^28 × E` (85 digits), in five gzipped blocks of 2,001 rows;
- `check_threshold_fixture.py` recomputes every row from the blocks;
- `check_conditioning.py` checks a file of optfix answers against the exact roots.

`optfix/tests/data/iv_threshold_codex.csv` is the five blocks concatenated in order, unzipped.
To run the standalone checker on it:

    mkdir blocks && split -l 2001 -d -a 1 optfix/tests/data/iv_threshold_codex.csv blocks/b
    for i in 0 1 2 3 4; do gzip -c blocks/b$i > blocks/conditioning-exact-$((i + 1)).csv.gz; done
    python3 tools/iv_fixture/codex/check_threshold_fixture.py blocks
