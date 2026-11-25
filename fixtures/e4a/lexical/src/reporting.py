"""One-line reporting over checkout totals."""

from checkout import checkout


def build_report(value: float) -> str:
    return f"total={checkout(value):.2f}"
