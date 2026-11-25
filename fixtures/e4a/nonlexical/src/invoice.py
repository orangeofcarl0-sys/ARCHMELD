"""Invoice totals."""

INVOICE_ADJUSTMENT_BPS = 1000


def invoice_total(value: float) -> float:
    return value * (10000 - INVOICE_ADJUSTMENT_BPS) / 10000
