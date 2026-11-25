"""EU VAT; unrelated to the storefront discount."""

VAT_BPS = 2000


def vat_for(value: float) -> float:
    return value * VAT_BPS / 10000
