# JholVM JS Conformance Corpus

Standalone JS programs plus their exact expected stdout, used to conformance-test
the JholVM engine from crates/jhol-vm.

## Layout
- `NN-name.js`        complete standalone program; observable output only via console.log
- `NN-name.expected` exact expected stdout, one line per console.log line

## Run
    sh run_corpus.sh node              # reference check against Node
    sh run_corpus.sh /path/to/jholvm   # conformance check against JholVM once built

Pass a single command that runs one JS file given as its argument. The script prints
PASS/FAIL per file and an "N/M passed" summary, and exits nonzero if any file fails.

## Conventions
- Deterministic: no Date, no Math.random, no object-address printing, no float noise.
- Integers and short decimals only. Classes live only in file 17; generators in 31.

Generate expected files with: `for f in [0-9][0-9]-*.js; do node "$f" > "${f%.js}.expected"; done`
