# `Some[Tr]` checks its argument against the trait `Tr` receives, not the
# trait `Tr` declares: `NW` is no `Writable`.
# expect: 'f' parameter 'Some[Tr]' has 'Writable' type, but value has type 'AnyStruct[NW]'
struct NW:
    def __init__(out self):
        pass


def f[Tr: type_of(AnyType)](t: Some[Tr]):
    pass


def main():
    f[Writable](NW())
