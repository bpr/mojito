# expect: unqualified access to struct parameter 'Ts'; use 'Self.Ts' instead
# A method body names the struct's own pack `Self.Ts` (`Self.Ts.length`), as
# upstream requires; the bare name is reserved for the struct header.
struct Bag[*Ts: Movable](
    Deinitable where Ts.all_conforms_to[Deinitable](),
    Movable,
):
    var storage: Tuple[*Self.Ts]

    def __init__(out self, var *args: *Self.Ts):
        self.storage = Tuple[*Self.Ts](*args^)

    def count(self) -> Int:
        return Ts.length


def main():
    var b = Bag[Int, Bool](1, True)
    print(b.count())
