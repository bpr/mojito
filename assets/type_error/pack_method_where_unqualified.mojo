# expect: unqualified access to struct parameter 'Ts'; use 'Self.Ts' instead
# A method's availability clause names the struct's own pack `Self.Ts`, as
# upstream requires; only the struct's own conformance clauses, where `Self`
# is not available, keep the bare name.
struct Bag[*Ts: Movable](
    Deinitable where Ts.all_conforms_to[Deinitable](),
    Movable,
):
    var storage: Tuple[*Self.Ts]

    def __init__(out self, var *args: *Self.Ts):
        self.storage = Tuple[*Self.Ts](*args^)

    def count(self) -> Int where Ts.all_conforms_to[Copyable]():
        return Self.Ts.length


def main():
    var b = Bag[Int, Bool](1, True)
    print(b.count())
