"""Analytics checks a basket total."""

from pricing import apply_discount

EXPECTED_TOTAL = 11110


def check_total() -> bool:
    return round(apply_discount(12345)) == EXPECTED_TOTAL
