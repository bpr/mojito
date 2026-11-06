# A compile-time application of a callable whose `where` clause reads a
# generic comptime alias declared above it checks the clause as the
# callee's signature emits it.
comptime N = 2
comptime IsN[n: Int] = n == N


def g[n: Int]() -> Int where IsN[n]:
    return n * 10


struct S[k: Int]:
    @staticmethod
    def get() -> Int where IsN[Self.k]:
        return Self.k


def take[k: Int]():
    print(k)


def main():
    take[g[2]()]()
    take[S[2].get()]()
