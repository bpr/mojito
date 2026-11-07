# The `assert` statement calls the standard library's `debug_assert`, whose
# default `assert_mode="none"` the default `ASSERT` mode (`safe`) skips: a
# failing assertion does not stop the program, but its condition and message
# are still evaluated, as at the pin. A user's own `debug_assert` does not
# capture the statement.


def debug_assert(cond: Bool, message: StringLiteral):
    print("user debug_assert")


def side(label: String) -> Bool:
    print("side", label)
    return False


def msg() -> String:
    print("msg")
    return "message"


struct Counter:
    var n: Int

    def __init__(out self, n: Int):
        self.n = n

    def check(self):
        assert self.n == 0, "counter is not zero"
        print("checked", self.n)


def positive[n: Int]() -> Int:
    assert n > 0, "n must be positive"
    return n


def main():
    var n = 2
    assert n == 3
    assert n == 3, "boom"
    assert n == 3, n
    assert n == 2, String("n is ", n)
    assert side("cond"), msg()
    assert (
        n == 4
    ), "multi-line"
    Counter(5).check()
    print(positive[0]())
    for i in range(3):
        assert i > 5, "loop"
    comptime if True:
        assert False, "comptime arm"
    print("ok")
