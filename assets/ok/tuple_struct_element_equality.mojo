# Tuple `==`/`!=` over user-struct elements, as operators and as explicit
# dunder calls; `Opaque` declares only `__eq__` (Equatable's default `__ne__`).
@fieldwise_init
struct Opaque(Equatable, Copyable, Movable):
    var value: Int
    def __eq__(self, other: Self) -> Bool:
        return self.value == other.value
def main():
    var t = (1, Opaque(2))
    var u = (1, Opaque(3))
    print(t == u, t != u, t == (1, Opaque(2)))
    print(t.__ne__(u), t.__eq__(t))
