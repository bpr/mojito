# A nested generic `def` whose binder's type differs from a generic callable
# bound's (`def[w: Bool]` at `def[n: Int]`) does not bind it: the bound's
# contract is the signature up to binder spelling, not up to binder type.
# expect: type 'def[w: Bool]() thin -> Int' for parameter 'F' does not conform to trait 'def[n: Int]() -> Int'
def apply[F: ImplicitlyCopyable & RegisterPassable & def[n: Int]() -> Int, //](f: F) -> Int:
    return f[3]()


def main():
    def body[w: Bool]() -> Int:
        return 1

    print(apply(body))
