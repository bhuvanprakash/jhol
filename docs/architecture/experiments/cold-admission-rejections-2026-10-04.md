# Cold admission experiments — rejected

Four-times cold target remains unmet. No runtime admission change retained.

## Causal mechanism

Existing four-request window covers headers and bodies. A local12-package200ms-header fixture showed request arrival spread413.34ms versus3.92ms when bypassed. Both layouts, SRI-pinned inputs and consumers passed. This proves admission-induced request waves in the synthetic fixture; it does not prove this is the dominant registry bottleneck.

Split header/body admission was rejected first: one real run took30770.38ms, followed by another stall. Actual cause unproven. Partial raw logs retained; overlapping CLI build makes those samples unsuitable for a performance claim. Source snapshot archived.

## Registry-cold rejection pilot

Three randomized interleaved rounds, fresh projects/home/tool caches, same React18.2/react-dom18.2/Vite5 lock. Harvest, prepared, daemon, JGrid and prebuilt reuse disabled. Staged extraction. OS/CDN caches and host load uncontrolled. No builds ran during this second timed pilot.

- Saved Jhol baseline median: 555.31ms.
- Experimental single transport admission: 543.56ms.
- Bun1.4.2: 628.63ms.
- Baseline/candidate ratio: 1.0216; paired bootstrap95% interval [0.9326706953005175, 1.041231643392058]. Three rounds insufficient for a general gain claim.
- Bun/candidate ratio: 1.1565; interval [0.9149287028553367, 1.47797385764686].4x gate red.
-9/9 scoped install checks and3/3 Vite builds passed. Jhol before/after trees, inventory and locks identical.
- Full core HTTP feature535 passed/2ignored; CLI30 passed; baselineHTTP25 passed. Memory watchdog/two jobs used; safe release builds peaked68% systemmemory. CPU observation includes quality subprocesses; install RSS unavailable.

## Outcome

Archive both failed candidates. Restore this pass's source edits to byte-identical saved current input; restore saved release binary9d7a6a78. Installed user binary untouched. Three papers unchanged; no Git writes or competitor implementation copied. Remaining118 authored recent-doc inventory entries fully reviewed; generated graph report excluded. Visible Antigravity chat/checkpoints complete; complete tool-result corpus not claimed.

Network fetch/extraction remains the measured dominant stage. Admission alone is not the demonstrated core bottleneck. Do not repeat this policy or rejected buffer/window/range tuning as an assumed speed fix.

[Raw observations, embedded runner, identities, hashes and checks](cold-header-admission-ab-20261004-141748.json).
