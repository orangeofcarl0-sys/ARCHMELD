"""Rate limiting; unrelated to the storefront discount."""

BURST = 1000


def window(limit: int) -> int:
    return limit * BURST
