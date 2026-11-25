"""Shipping rates; unrelated to the storefront discount."""

BASE_RATE_BPS = 1000  # shipping, not pricing


def shipping_fee(weight_grams: int) -> int:
    return max(500, weight_grams * BASE_RATE_BPS // 1000)
