# A display of non-`Deinitable` elements is a temporary the statement cannot
# destroy: borrowing it to read an element abandons it.
# expect: '(expression temporary)' abandoned without being explicitly destroyed: Use `deinit_with()` to explicitly destroy an `Array` of non-`Deinitable` elements
struct Res(Deinitable where False, Movable):
    var id: Int

    def __init__(out self, id: Int):
        self.id = id


def main():
    print([Res(1)][0].id)
