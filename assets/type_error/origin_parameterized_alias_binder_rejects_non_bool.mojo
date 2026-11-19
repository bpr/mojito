# expect: origin mutability for 'o' must be Bool
# A parameterized alias of `Origin` applied to a non-`Bool` mutability is
# rejected, as `Origin[mut=3]` is.
comptime O[m: Bool] = Origin[mut=m]


def f[o: O[3]](ref[o] x: Int):
    pass


def main():
    var a = 1
    f(a)
    print(a)
