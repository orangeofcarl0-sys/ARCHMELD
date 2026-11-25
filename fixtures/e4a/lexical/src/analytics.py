"""Analytics checks the old price for a fixed basket."""

from pricing import apply_discount


def check_price() -> bool:
    return apply_discount(100) == 90
