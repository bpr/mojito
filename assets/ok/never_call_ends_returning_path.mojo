# A call whose result is `Never` ends its path for return analysis, named
# result and field initialization, and ownership, as at the pin: a user
# `-> Never` function (whose own body must not reach its end), a
# `raises -> Never` callee, `std.os.abort` in an `else` arm, and an `out`
# result left unset on the aborting path.
from std.os import abort


def die(x: Int) -> Never:
    print("dying", x)
    while True:
        pass


def fail(message: String) raises -> Never:
    raise Error(message)


def positive(x: Int) -> Int:
    if x > 0:
        return x
    die(x)


def checked(x: Int) raises -> Int:
    if x >= 0:
        return x * 2
    fail("negative")


def halve(x: Int) -> Int:
    if x % 2 == 0:
        return x // 2
    else:
        abort("odd")


def named(x: Int, out result: String):
    if x > 0:
        result = String(x)
        return
    abort("not positive")


def main():
    print(positive(4))
    print(halve(10))
    print(named(7))
    try:
        print(checked(3))
        print(checked(-1))
    except e:
        print("caught", e)
