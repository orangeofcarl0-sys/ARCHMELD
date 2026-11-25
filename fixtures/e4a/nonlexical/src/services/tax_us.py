"""US sales tax; unrelated to the storefront discount."""

TAX_BPS = 825


def tax_for(value: float) -> float:
    return value * TAX_BPS / 10000
