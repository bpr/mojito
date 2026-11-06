# expect: invalid call to 'g': violated constraint
# A compile-time application whose arguments violate the callee's `where`
# clause, which reads a generic comptime alias, is rejected.
comptime N = 2
comptime IsN[n: Int] = n == N


def g[n: Int]() -> Int where IsN[n]:
    return n * 10


def take[k: Int]():
    print(k)


def main():
    take[g[3]()]()
