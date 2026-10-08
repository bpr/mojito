# A call result whose struct is not `Deinitable`, read through a field and
# never consumed, is abandoned at the end of its statement.
# expect: '(expression temporary)' abandoned without being explicitly destroyed: type 'Res' does not conform to 'Deinitable' and must be explicitly destroyed
struct Res(Deinitable where False, Movable):
    var id: Int

    def __init__(out self, id: Int):
        self.id = id


def mk() -> Res:
    return Res(1)


def main():
    print(mk().id)
