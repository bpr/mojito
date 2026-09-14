# expect: unqualified access to struct parameter 'Ts'; use 'Self.Ts' instead
# A struct's own pack is spelled `Self.Ts` in a field type inside its body,
# as upstream requires; the bare spread is not in scope there.
struct Bag[*Ts: Movable](
    Deinitable where Ts.all_conforms_to[Deinitable](),
    Movable,
):
    var storage: Tuple[*Ts]

    def __init__(out self, var *args: *Self.Ts):
        self.storage = Tuple[*Self.Ts](*args^)


def main():
    var b = Bag[Int, Bool](1, True)
    print(b.storage[0])
