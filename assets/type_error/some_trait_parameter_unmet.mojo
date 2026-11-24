# `Some[Tr]` checks its argument against the trait `Tr` receives, not the
# trait `Tr` declares: `NW` is no `Writable`.
# expect: type 'NW' for parameter 'Some[Tr]' does not conform to trait 'Writable'
struct NW:
    def __init__(out self):
        pass


def f[Tr: type_of(AnyType)](t: Some[Tr]):
    pass


def main():
    f[Writable](NW())
