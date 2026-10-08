# expect: recursively requires itself
# A module constant whose initializer calls a function that reads a
# constant derived from the first is a cycle in the parameter domain.
def f(n: Int) -> Int:
    return n + B


comptime A = f(1)
comptime B = A + 1


def main():
    print(A)
