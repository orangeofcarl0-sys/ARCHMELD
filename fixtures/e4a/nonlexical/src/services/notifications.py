"""Notification retries; unrelated to the storefront discount."""

RETRY_DELAY_MS = 1000


def retry_delay(attempt: int) -> int:
    return RETRY_DELAY_MS * attempt
