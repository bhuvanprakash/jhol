#!/bin/sh
# Regenerate adversarial expectations with node.
cd "$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
for f in adv-*.js; do
  node "$f" > "${f%.js}.expected" 2>&1
done
echo "regenerated $(ls adv-*.expected | wc -l | tr -d " ") expectations"
