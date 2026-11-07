# `IntLiteral` declares upstream's non-negative-exponent `__pow__`, whose
# `where (type_of(exp)() >= 0)` folds `__ge__`'s attribute at each call, so a
# literal power is the declared dunder and its type carries the value.
def nonnegative(exp: IntLiteral[_]) -> Int where (type_of(exp)() >= 0):
    return Int(exp)


def main():
    print((2).__pow__(10), 2 ** 10)
    comptime k = 3 ** 4
    var t: type_of(2 ** 5) = {}
    print(k, t, (-2) ** 3, 0 ** 0)
    print(nonnegative(7))
