# expect: cannot infer type parameter 'n'
# A value parameter is inferred from a direct reference only. `Buf[n + 1]`
# against `Buf[4]` is a residual equation no solver inverts, here or at the
# pin; the explicit `take[3](...)` is the supported spelling.
struct Buf[n: Int](Copyable, Movable):
    var value: Int

    def __init__(out self, value: Int):
        self.value = value


def take[n: Int](x: Buf[n + 1]) -> Int:
    return n


def main():
    print(take(Buf[4](7)))
