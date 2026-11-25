"""Currency conversion; unrelated to the storefront discount."""

CENTS_PER_UNIT = 100


def to_cents(value: float) -> int:
    return round(value * CENTS_PER_UNIT)
