# expect: expected Buf[n ** 2], found Buf[n * n]
# A symbolic power is a deferred atom and is never expanded: the pinned Mojo
# does not equate `n ** 2` with `n * n`.
struct Buf[n: Int](Copyable, Movable):
    var value: Int

    def __init__(out self, value: Int):
        self.value = value


def square[n: Int](var x: Buf[n * n]) -> Buf[n ** 2]:
    return x^


def main():
    print(1)
