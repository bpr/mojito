# An origin binder whose `mut` is left unbound (`Origin[mut=_]`, bare
# `Origin`) or names an inferred `Bool` binder is classified by its
# annotation's type, and a reference through it reads like any other.


def read_any[o: Origin[mut=_]](ref[o] x: Int) -> Int:
    return x


def read_bare[o: Origin](ref[o] x: Int) -> Int:
    return x


def pick[m: Bool, //, o: Origin[mut=m]](ref[o] x: Int) -> ref[o] Int:
    return x


def bump[o: Origin[mut=True]](ref[o] x: Int):
    x += 1


def main():
    var a = 1
    print(read_any(a))
    print(read_bare(a))
    bump(a)
    pick(a) += 5
    print(a)
