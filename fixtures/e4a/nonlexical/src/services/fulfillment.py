"""Parcel handling; unrelated to the storefront discount."""

PARCEL_WEIGHT_GRAMS = 1000


def is_oversized(weight_grams: int) -> bool:
    return weight_grams > PARCEL_WEIGHT_GRAMS
