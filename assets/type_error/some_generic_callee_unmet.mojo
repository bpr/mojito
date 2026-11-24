# A generic callee's `Some[Writable]` refuses an argument that is no
# `Writable`, as a declared `[T: Writable]` binder does.
# expect: type 'NW' for parameter 'Some[Writable]' does not conform to trait 'Writable'
struct NW:
    def __init__(out self):
        pass


def f[n: Int](t: Some[Writable]):
    pass


def main():
    f[1](NW())
