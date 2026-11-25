"""Loyalty points; unrelated to the storefront discount."""

POINTS_PER_DOLLAR = 1000


def points_for(amount: float) -> int:
    return int(amount * POINTS_PER_DOLLAR)
