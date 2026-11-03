# Only a register-passable `@__nonmaterializable` struct materializes: a
# memory-only one stays itself at an inferred binding, so its target's
# field is no member of it.
# expect: type 'Lit' has no field 'v'
struct Big(ImplicitlyCopyable):
    var v: Int

    @implicit
    def __init__(out self, lit: Lit):
        self.v = 7


@__nonmaterializable(Big)
struct Lit(ImplicitlyCopyable):
    def __init__(out self):
        pass


def main():
    var x = Lit()
    print(x.v)
