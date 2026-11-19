# expect: expected Bool, found Int
# An origin binder's `mut` naming a non-`Bool` binder is rejected where
# the list is classified, as upstream's binding of `Origin`'s `mut` is.


def f[n: Int, o: Origin[mut=n]](ref[o] x: Int) -> Int:
    return x


def main():
    var a = 1
    print(f[1](a))
