"""Cache TTL; unrelated to the storefront discount."""

TTL_SECONDS = 1000


def expiry(now: int) -> int:
    return now + TTL_SECONDS
