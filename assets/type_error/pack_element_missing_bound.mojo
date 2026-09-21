# expect: does not conform to trait 'Writable'
# An element of a pack may use only what the pack's declared bound and the
# enclosing declaration's conjunctive `where` guarantee. A struct-header
# conditional conformance does not reach into the method that implements it:
# `write_to` needs its own `where conforms_to(Self.Ts.values, Writable)`.
struct Bag[*Ts: Movable](
    Deinitable where Ts.all_conforms_to[Deinitable](),
    Movable,
    Writable where Ts.all_conforms_to[Writable](),
):
    var storage: Tuple[*Self.Ts]

    def __init__(out self, var *args: *Self.Ts):
        self.storage = Tuple[*Self.Ts](*args^)

    def write_to(self, mut writer: Some[Writer]):
        comptime for i in range(Self.Ts.length):
            writer.write(self.storage[i])


def main():
    print(Bag[Int, String](1, "two"))
