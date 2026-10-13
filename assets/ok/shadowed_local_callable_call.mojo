# A call through a local callable calls the binding its name resolves to:
# an inner `var f` shadows the outer one only inside its block, and once a
# block's `var g` is out of scope `g` names the module function again.


def one() -> Int:
    return 1


def two() -> Int:
    return 2


def g() -> Int:
    return 5


def add(x: Int) -> Int:
    return x + 10


def add_more(x: Int) -> Int:
    return x + 20


def main():
    var f = one
    for _ in range(1):
        var f = two
        print(f())
    print(f())
    var n = 1
    if n > 0:
        var f = two
        print(f())
    print(f())

    var fs = [add]
    for _ in range(1):
        var fs = [add_more]
        print(fs[0](1))
    print(fs[0](1))

    for _ in range(1):
        var g = one
        print(g())
    print(g())
    var h = g
    print(h())
