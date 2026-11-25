"""Experiment traffic split; unrelated to the storefront discount."""

CONTROL_WEIGHT = 0.9


def in_control(bucket: float) -> bool:
    return bucket < CONTROL_WEIGHT
