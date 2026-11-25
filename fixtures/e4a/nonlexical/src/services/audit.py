"""Audit sampling; unrelated to the storefront discount."""

SAMPLE_SIZE = 1000


def sample_every(index: int) -> bool:
    return index % SAMPLE_SIZE == 0
