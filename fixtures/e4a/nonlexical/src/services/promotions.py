"""Promotional campaign multipliers; unrelated to the contractual rate."""

PROMO_MULTIPLIER = 0.9  # campaign traffic, not the storefront discount


def apply_promo(value: float) -> float:
    return value * PROMO_MULTIPLIER
