"""Forecast horizon; unrelated to the storefront discount."""

DEFAULT_HORIZON_DAYS = 90


def horizon() -> int:
    return DEFAULT_HORIZON_DAYS
