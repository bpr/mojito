# An instance's compile-time ops are elaborated in order, as upstream's work
# list reaches them: a `comptime if` is decided and its taken arm spliced
# before any op in it is reached, so a `comptime for` header, a nested
# condition's application, a local `comptime` binding, or an `elif` past a
# taken arm in an arm the instance never takes is never evaluated, and
# `10 // n` at `n == 0` divides nothing.
def total[n: Int]() -> Int:
    var s = 0
    comptime if n > 0:
        comptime for i in range(10 // n):
            s += i
    return s


def guarded[n: Int]():
    comptime if n > 100:
        comptime for z in [10 // (n - 4)]:
            print(z)
    print("guarded", n)


def big(n: Int) -> Bool:
    return 10 // n > 2


def nested[n: Int]() -> Int:
    var s = 0
    comptime if n > 0:
        comptime if big(n):
            s += 1
    return s


def bound[n: Int]() -> Int:
    var s = 0
    comptime if n > 0:
        comptime xs = [10 // n, 2]
        comptime y = xs[0]
        s += y
    return s


def chained[n: Int]() -> Int:
    var s = 0
    comptime if n == 0:
        s = -1
    elif 10 // n > 1:
        s = 2
    return s


def returns_first[n: Int]() -> Int:
    comptime if n == 0:
        return 0
    comptime for _ in range(10 // n):
        pass
    return 1


def main():
    print(total[0](), total[2]())
    guarded[4]()
    print(nested[0](), nested[2]())
    print(bound[0](), bound[2]())
    print(chained[0](), chained[2]())
    print(returns_first[0](), returns_first[2]())
