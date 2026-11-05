# A pack of callables is its `ParameterList`, as any value pack is: each
# element read is the bound function, and the pack takes no frame slot.


def a():
    print("a")


def b():
    print("b")


def inc(x: Int) -> Int:
    return x + 1


def dbl(x: Int) -> Int:
    return x * 2


def run_each[*fs: def() thin -> None]():
    comptime for i in range(len(fs)):
        fs[i]()


def count[*fs: def() thin -> None]() -> Int:
    return len(fs)


def call_bound[*fs: def() thin -> None]():
    comptime for i in range(len(fs)):
        var g = fs[i]
        g()


def last[*fs: def() thin -> None]():
    comptime g = fs[1]
    g()


def total[*fs: def(Int) thin -> Int](x: Int) -> Int:
    var sum = 0
    comptime for i in range(len(fs)):
        sum += fs[i](x)
    return sum


def main():
    run_each[a, b]()
    print(count[a, b, a]())
    call_bound[b, a]()
    last[a, b]()
    print(total[inc, dbl](5))
    print(total[dbl](3))
