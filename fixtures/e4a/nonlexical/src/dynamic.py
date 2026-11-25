"""Runtime-resolved pricing call."""

import pricing


def dynamic_discount(value: float) -> float:
    fn = getattr(pricing, "apply_discount")
    return fn(value)
