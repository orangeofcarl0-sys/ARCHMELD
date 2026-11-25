"""Weekly reporting; EXPECTED_ORDERS is an unrelated volume target."""

EXPECTED_ORDERS = 11110


def order_variance(actual: int) -> int:
    return actual - EXPECTED_ORDERS
