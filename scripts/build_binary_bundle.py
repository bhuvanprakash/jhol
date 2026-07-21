#!/usr/bin/env python3
import json
from pathlib import Path

root = Path('binary-packages')
index_path = root / 'index.json'
if not index_path.exists():
    raise SystemExit('binary-packages/index.json missing')

index = json.loads(index_path.read_text())
bundle_path = root / 'bundle.bin'
bundle_index_path = root / 'bundle-index.json'

bundle_index = {}
offset = 0
with bundle_path.open('wb') as out:
    # deterministic order by hash for stable bundle layout
    hashes = sorted(set(index.values()))
    for h in hashes:
        src = root / f'{h}.jhol'
        if not src.exists():
            continue
        data = src.read_bytes()
        out.write(data)
        bundle_index[h] = {'offset': offset, 'len': len(data)}
        offset += len(data)

bundle_index_path.write_text(json.dumps(bundle_index, indent=2) + '\n')
print(f'wrote {bundle_path} ({offset} bytes), entries={len(bundle_index)}')
