# A conditional conformance spelled with upstream's bare-pack receiver
# (`Writable where Ts.all_conforms_to[Writable]()`) is unavailable when an
# element does not conform: the gated `write_to` is dropped and printing the
# bag rejects.
# expect: does not conform to trait 'Writable'
from std.builtin.tuple import Tuple

struct Opaque(Copyable, Movable):
    var n: Int

    def __init__(out self, n: Int):
        self.n = n

struct Bag[*Ts: Copyable & Movable & Deinitable](
    Copyable,
    Movable,
    Writable where Ts.all_conforms_to[Writable](),
):
    var storage: Tuple[*Self.Ts]

    def __init__(out self, var *args: *Self.Ts):
        self.storage = Tuple[*Self.Ts](*args^)

    def write_to(self, mut writer: Some[Writer]) where Self.Ts.all_conforms_to[Writable]():
        comptime for i in range(Self.Ts.length):
            writer.write(self.storage[i])

def main():
    var b = Bag[Int, Opaque](1, Opaque(2))
    print(b)
