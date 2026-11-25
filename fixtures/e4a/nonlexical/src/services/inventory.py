"""Inventory thresholds; RESTOCK_THRESHOLD is an unrelated count."""

RESTOCK_THRESHOLD = 11110


def needs_restock(units: int) -> bool:
    return units < RESTOCK_THRESHOLD
