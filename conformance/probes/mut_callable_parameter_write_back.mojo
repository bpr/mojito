# Assigning a function value to a `mut` parameter of callable type writes it
# back to the caller's binding: the pin and the VM both print 2. Natively the
# write does not reach the caller, which still calls `one` and prints 1
# (roadmap 2.4). Promote to `assets/ok` once the native run prints 2.
def one() -> Int:
    return 1


def two() -> Int:
    return 2


def keep(mut f: def() thin -> Int):
    f = two


def main():
    var f = one
    keep(f)
    print(f())
