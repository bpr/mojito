# A conditional conformance spelled with upstream's bare-pack receiver
# (`Writable where Ts.all_conforms_to[Writable]()`) is unavailable when an
# element does not conform: the gated `write_to` is dropped and printing the
# bag rejects.
# expect: expected Writable
from std.collections.tuple import Tuple

struct Opaque(Copyable, Movable):
    var n: Int

    def __init__(out self, n: Int):
        self.n = n

struct Bag[*Ts: Copyable & Movable](
    Copyable,
    Movable,
    Writable where Ts.all_conforms_to[Writable](),
):
    var storage: Tuple[*Ts]

    def __init__(out self, var *args: *Ts):
        self.storage = Tuple[*Ts](*args^)

    def write_to(self, mut writer: Some[Writer]) where Ts.all_conforms_to[Writable]():
        comptime for i in range(Ts.length):
            writer.write(self.storage[i])

def main():
    var b = Bag[Int, Opaque](1, Opaque(2))
    print(b)
