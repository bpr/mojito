# A value-returning body whose arm ends in a `while True:` loop with no
# `break` of its own: no path reaches the end of the arm, so the function
# returns on every path, as at the pin.
def h[n: Int](x: Int) -> Int:
    comptime if n == 1:
        return x
    else:
        while True:
            pass


def main():
    print(h[1](4))
