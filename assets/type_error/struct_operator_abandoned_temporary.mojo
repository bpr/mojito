# expect: '(expression temporary)' abandoned without being explicitly destroyed: type 'Res' does not conform to 'Deinitable' and must be explicitly destroyed
# A struct operator's temporary operand is the caller's to destroy once the
# dunder returns; a linear one cannot be destroyed implicitly.
struct Res(Deinitable where False, Movable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __lt__(self, other: Self) -> Bool:
        return self.v < other.v

    def destroy(deinit self):
        pass


def mk() -> Res:
    return Res(1)


def main():
    var a = Res(2)
    print(mk() < a)
    a^.destroy()
