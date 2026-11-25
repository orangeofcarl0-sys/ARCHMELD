from pricing import apply_discount


def checkout(value):
    return apply_discount(value) + 2
