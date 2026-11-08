# expect: '@always_inline("builtin")' does not support MLIR operation lit.ref.immut
# A builtin body cannot pass an argument to a positional collector, which
# takes it by reference, before the call itself is judged.
def total(*values: Int) -> Int:
    return 1


@always_inline("builtin")
def wrapped(n: Int) -> Int:
    return total(n)


def main():
    print(1)
