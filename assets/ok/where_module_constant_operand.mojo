# A module constant in a `where` clause or a predicate alias's body is its
# value, not a parameter of that name.
comptime N = 2

comptime IsN[n: Int] = n == N


def f[n: Int]() -> Int where n == N:
    return n


def g[n: Int]() -> Int where IsN[n]:
    return n * 10


def main():
    print(f[2]())
    print(g[2]())
    comptime if IsN[2]:
        print("two is N")
