# Comparison and `~` over `comptime for` variables and value parameters
# alone. The elaborator folds each name to the instance's literal, so the
# operator folds too: an inversion is an `IntLiteral` that materializes to
# the template's `Int` (wrapping past its range, as the pin does), and a
# comparison a `Bool` in both checks. Every instance derives those facts
# from the checked template instead of being checked again.
def take(b: Bool) -> Int:
    return 1 if b else 0


def tests[n: Int]() -> Int:
    var acc = 0
    comptime for i in range(n):
        print(~i, ~(i * 2), -~i, i * 2 < 5, (i + 1) // 2 == i)
        acc += take(i >= 2) + ~i
    return acc


def flags[n: Int]() -> Bool:
    var last = False
    comptime for i in range(n):
        var even = i % 2 == 0
        last = even
    return last


def wraps[n: Int]() -> Int:
    var acc = 0
    comptime for i in range(n):
        acc += (i + 1) * 4611686018427387904 * 4 + ~i
    return acc


def scaled[T: Copyable, n: Int](x: Int) -> Int:
    var r = x
    if n > 2:
        r = x * n
    if x < n:
        r += 1
    return r


def main():
    print(tests[3]())
    print(flags[4](), flags[5]())
    print(wraps[2]())
    print(scaled[Int, 3](4), scaled[String, 1](0))
