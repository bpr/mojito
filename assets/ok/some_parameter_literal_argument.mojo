# An existential `Some[Trait]` parameter is the anonymous `[T: Trait]` binder
# the argument deduces, so a literal argument materializes (`StringLiteral`
# to `String`) or defaults (`IntLiteral` to `Int`, `FloatLiteral` to
# `Float64`) there exactly as at a named binder, at a free function and at a
# method alike.


def hashed(x: Some[Hashable]) -> UInt64:
    return hash(x)


def show(x: Some[Writable]):
    print(x)


@fieldwise_init
struct Hasher:
    var salt: Int

    def digest(self, x: Some[Hashable]) -> UInt64:
        return hash(x)


def main():
    print(hashed("abc") == hash(String("abc")))
    print(hashed(3) == hash(Int(3)))
    show("hi")
    show(3)
    show(2.5)
    show(True)
    var h = Hasher(1)
    print(h.digest("abc") == hashed(String("abc")))
    print(h.digest(7) == hashed(Int(7)))
