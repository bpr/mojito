# A `mut` parameter is the caller's storage: `x^` takes the value out through
# the reference, and the body writes a value back before every return. A
# write-back destroys what an earlier one stored, a path that never
# transferred leaves the caller's value in place, and a trivial register value
# transfers as a copy.
def take(mut x: String) -> String:
    var r = x^
    x = String("b")
    return r^


def swap(mut x: String, mut y: String):
    var t = x^
    x = y^
    y = t^


def refill(mut x: String, fill: Bool) -> String:
    var r = String("kept")
    if fill:
        r = x^
        x = String("filled")
    return r^


def twice(mut x: String) -> String:
    var r = x^
    x = String("first")
    x = String("second")
    return r^


def recover(mut x: String) raises -> String:
    var r = x^
    try:
        raise Error("lost")
    except _e:
        x = String("back")
    return r^


def count(mut n: Int) -> Int:
    return n^


def main() raises:
    var s = String("a")
    print(take(s), s)
    var t = String("t")
    swap(s, t)
    print(s, t)
    print(refill(s, True), s)
    print(refill(s, False), s)
    print(twice(s), s)
    print(recover(s), s)
    var n = 3
    print(count(n), n)
