"""Pricing rules for the storefront."""


def apply_discount(value: float) -> float:
    return value * 0.9


def list_price(value: float) -> float:
    return apply_discount(value) + 5
