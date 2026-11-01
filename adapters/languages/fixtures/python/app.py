from pkg.util import helper as h


def dynamic(obj, name):
    return getattr(obj, name)()


def main(value):
    return h(value)
