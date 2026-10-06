# expect: type 'NoCopy' for parameter '*Ts' does not conform to trait 'Copyable'
struct NoCopy(Movable):
    var x: Int

    def __init__(out self, x: Int):
        self.x = x


struct Pair[*Ts: Copyable & Movable & Deinitable](Copyable, Movable):
    var storage: Tuple[*Self.Ts]

    def __init__(out self, var *args: *Self.Ts):
        self.storage = Tuple(*args^)


def main():
    var p = Pair[NoCopy, Int](NoCopy(1), 2)
    print(p.storage[1])
