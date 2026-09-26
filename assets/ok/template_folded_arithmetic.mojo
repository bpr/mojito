# Integer arithmetic over `comptime for` variables and literals alone. The
# elaborator folds each name to the instance's literal, so the operator folds
# too and the whole expression is an `IntLiteral` that materializes to the
# template's `Int`; every instance derives those facts from the checked
# template instead of being checked again.
def bump(x: Int) -> Int:
    return x + 1


def own(var x: Int) -> Int:
    x += 1
    return x


def nested[n: Int, m: Int]() -> Int:
    var sum = 0
    comptime for i in range(n):
        comptime for j in range(m):
            sum += i * 10 + j
    return sum


def mixed[n: Int]() -> Int:
    var acc = 0
    comptime for i in range(n):
        print(i * 10)
        acc += bump(i * 2 + 1) + own(i - 1)
        var x = i * (2 * 3)
        acc = acc * 2 + i * 10
        acc += x - (-i)
        acc += i // 2 + i % 3 + (i << 1) + i**2
    return acc


def main():
    print(nested[2, 3](), nested[0, 1]())
    print(mixed[0]())
    print(mixed[3]())
