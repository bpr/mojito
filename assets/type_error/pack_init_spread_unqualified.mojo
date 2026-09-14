# expect: unqualified access to struct parameter 'Ts'; use 'Self.Ts' instead
# A pack constructor's variadic parameter spells the struct's own pack
# `*Self.Ts`, as upstream requires; the bare `*Ts` is not in scope in a
# method signature.
struct Bag[*Ts: Movable](
    Deinitable where Ts.all_conforms_to[Deinitable](),
    Movable,
):
    var storage: Tuple[*Self.Ts]

    def __init__(out self, var *args: *Ts):
        self.storage = Tuple[*Self.Ts](*args^)


def main():
    var b = Bag[Int, Bool](1, True)
    print(b.storage[0])
