from pkg.util import helper


def main(value):
    return helper(value) + 1


class Box:
    def __init__(self, value):
        self.value = value
