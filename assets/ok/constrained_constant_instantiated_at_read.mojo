# A constrained constant is a generator type: an unread one is never
# checked, so neither a false clause nor one applying a function stops the
# program, and a read proves its clauses where it instantiates it at `Int`.
def f(x: Int) -> Int:
    return x + 1


comptime Big: Int where (f(1) > 5, "big only") = f(1)
comptime Never: Int where (1 > 5, "never") = 1
comptime Small: Int where (1 < 5, "small") = 7


def show[n: Int]():
    print(n)


def main():
    var x: Int = Small
    print(x)
    print(Small + 1)
    show[Small]()
