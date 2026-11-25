"""Invoice totals must agree with the pricing contract."""

EXPECTED_DISCOUNT = 0.9


def invoice_total(value: float) -> float:
    return value * EXPECTED_DISCOUNT
