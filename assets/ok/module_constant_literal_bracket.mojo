# A module string, float, or Bool constant is its literal's value as a
# bracket argument, its `+` and its `.value` folding as for the literal.

comptime KGENString = __mlir_type.`!kgen.string`
comptime lit = "So"
comptime F = 2.5
comptime B = True


def bind[s: StringLiteral]():
    print(s)


def kbind[s: KGENString](label: String):
    print(label)


def gf[x: Float64]():
    print(x)


def gb[b: Bool]():
    print(b)


struct Tagged[s: StringLiteral]:
    @staticmethod
    def show():
        print(Self.s)


def main():
    bind[lit]()
    bind[lit + "me"]()
    kbind[lit.value]("module")
    kbind[(lit + "me").value]("concat")
    gf[F]()
    gb[B]()
    Tagged[lit].show()
