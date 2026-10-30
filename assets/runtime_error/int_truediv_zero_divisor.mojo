# expect: integer division or modulo by zero
# A run-time integer `/` by zero traps, as the pin's `pop.div` does.
@no_inline
def divisor(n: Int) -> Int:
    return n - n


def main():
    var x = 10 / divisor(3)
    print(x)
