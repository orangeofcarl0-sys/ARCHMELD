"""Metric buckets; unrelated to the storefront discount."""

HISTOGRAM_BUCKETS = [90, 900, 9000]


def bucket(value: float) -> int:
    for bound in HISTOGRAM_BUCKETS:
        if value <= bound:
            return bound
    return HISTOGRAM_BUCKETS[-1]
