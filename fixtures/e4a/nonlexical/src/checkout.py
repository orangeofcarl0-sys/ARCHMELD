"""Checkout adds a handling fee to the discounted price."""

from pricing import apply_discount


def checkout(value: float) -> float:
    return apply_discount(value) + 2
