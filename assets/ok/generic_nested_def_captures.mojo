# A generic nested `def` that captures specializes per call and runs natively
# as on the VM: its environment — an enclosing value parameter read without
# naming it, captured locals (heap-owning included), a `mut` capture, a
# captured argument of a generic function — passes by reference into each
# instance, including calls in a loop and inside a `try` region.


def scaled[n: Int]() -> Int:
    return n


def outer[n: Int]() -> Int:
    def inner[k: Int]() -> Int:
        return scaled[k + 1]() * n

    return inner[10]() + inner[20]()


def locals() -> Int:
    var x = 5
    var ys: List[Int] = [1, 2, 3]

    def inner[k: Int]() {x, ys} -> Int:
        return x * k + len(ys)

    var total = inner[1]()
    x = 7
    for _ in range(2):
        total += inner[2]()
    return total


def show[T: Writable](prefix: String, value: T):
    def emit[n: Int]() {prefix}:
        print(prefix, n)

    emit[1]()
    emit[2]()
    print(value)


def bump_outer() -> Int:
    var count = 0

    def bump[step: Int]() {mut count}:
        count += step

    bump[3]()
    bump[4]()
    return count


def typed[T: Copyable & Writable](value: T):
    var tag = String("tag")

    def echo[U: Writable](u: U) {tag}:
        print(tag, u)

    echo(value)
    echo(42)


def raising() raises -> Int:
    var base = 10

    def check[k: Int]() raises {base} -> Int:
        if k > 100:
            raise Error("too big")
        return base + k

    try:
        return check[1]() + check[200]()
    except e:
        print(e)
    return check[2]()


def main() raises:
    print(outer[1](), outer[2]())
    print(locals())
    show("hi", 3)
    print(bump_outer())
    typed(1.5)
    print(raising())
