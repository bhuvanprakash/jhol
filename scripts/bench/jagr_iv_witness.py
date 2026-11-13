#!/usr/bin/env python3
"""Exhaustive check of path-prefix invalidation in Paper IV's finite model."""

import itertools
import json


def first_visible(slots):
    return next((version for version in slots if version), 0)


def checked_prefix(slots):
    for index, version in enumerate(slots):
        if version:
            return slots[:index + 1]
    return slots


def evaluate():
    # Ordered nearest to farthest. Zero means an absent provider.
    layouts = list(itertools.product((0, 1, 2), repeat=3))
    ranges = ({1}, {2}, {1, 2})
    counts = {
        "layouts": len(layouts),
        "comparisons": 0,
        "prefix_reuses": 0,
        "prefix_false_reuses": 0,
        "present_provider_only_reuses": 0,
        "present_provider_only_false_reuses": 0,
        "present_provider_only_false_accepts": 0,
    }
    for old, new, allowed in itertools.product(layouts, layouts, ranges):
        before = first_visible(old) in allowed
        after = first_visible(new) in allowed
        prefix = checked_prefix(old)
        counts["comparisons"] += 1
        if prefix == new[:len(prefix)]:
            counts["prefix_reuses"] += 1
            counts["prefix_false_reuses"] += before != after
        if prefix[-1] and prefix[-1] == new[len(prefix) - 1]:
            counts["present_provider_only_reuses"] += 1
            counts["present_provider_only_false_reuses"] += before != after
            counts["present_provider_only_false_accepts"] += before and not after
    assert counts["prefix_false_reuses"] == 0
    assert counts["present_provider_only_false_accepts"] > 0
    return counts


if __name__ == "__main__":
    print(json.dumps(evaluate(), indent=2))
