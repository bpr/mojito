# expect: unqualified access to struct parameter 'Ts'; use 'Self.Ts' instead
# A struct's `comptime` member names the struct's own pack `Self.Ts`
# (upstream's `comptime element_types = Self.Ts`); the bare name is reserved
# for the struct header.
struct Bag[*Ts: Movable](
    Deinitable where Ts.all_conforms_to[Deinitable](),
    Movable,
):
    comptime element_types = Ts
    var storage: Tuple[*Self.Ts]

    def __init__(out self, var *args: *Self.Ts):
        self.storage = Tuple[*Self.Ts](*args^)


def main():
    var b = Bag[Int, Bool](1, True)
    print(b.storage[0])
