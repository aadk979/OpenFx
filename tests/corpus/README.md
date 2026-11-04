# Test corpora

`thingi10k-200.csv` lists a 200-model subset of [Thingi10K](https://ten-thousand-models.appspot.com/) (Zhou et al., 2020) used by the torture harness. Regenerate it with `tools/thingi10k/select_manifest.py`.

## Licensing

Each Thingi10K model has its own license, recorded per row. **Meshes are never committed to this repository.** They download into the git-ignored `corpus/` directory, and only IDs, hashes, license names, and defect labels are tracked here. The subset is limited to Creative Commons Attribution, Attribution-ShareAlike, public domain, and BSD models, because the harness transforms the models.

## Running

```sh
python tools/thingi10k/fetch.py
cargo run --release -p pcc-torture -- run
```

The summary prints to the terminal, and per-run records go to the git-ignored `reports/` directory. Run `cargo run --release -p pcc-torture -- --help` for options.

## Selection

Models are stratified by the defects in the dataset's own metadata, with half of each stratum being its most severe examples and half an unbiased sample. Models over 150,000 faces and non-STL models are excluded. The `clean` stratum is a control group: it separates operator bugs from legitimate rejection of broken input.

Bugs found this way are reduced to small synthetic regression tests in `crates/pcc-torture/tests/`, which run in normal CI without any download.
