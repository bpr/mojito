# A method clone derived from its checked template still owes `Movable` at
# each `^` transfer of a value of a parameter type: the instance below opts
# out, so its clone keeps the clone check and reports the failed conformance.
# expect: does not conform to trait 'Movable'
struct Pinned(Deinitable, Movable where False):
    var id: Int

    def __init__(out self, id: Int):
        self.id = id


@fieldwise_init
struct Holder[T: Deinitable]:
    var uses: Int

    def forward(mut self, var item: Self.T) -> Self.T:
        self.uses += 1
        return item^


def main():
    var h = Holder[Int](0)
    print(h.forward(3))
    var p = Holder[Pinned](0)
    print(p.forward(Pinned(4)).id)
