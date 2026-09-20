# expect: expected Buf[n + 2], found Buf[n + 1]
# Different canonical nodes are not established equal, so the conversion is
# rejected at the generic declaration, before any instantiation.
struct Buf[n: Int](Copyable, Movable):
    var value: Int

    def __init__(out self, value: Int):
        self.value = value


def wrong[n: Int](var x: Buf[n + 1]) -> Buf[n + 2]:
    return x^


def main():
    print(1)
