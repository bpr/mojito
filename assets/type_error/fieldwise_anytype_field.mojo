# expect: field 'a' has non-'Deinitable' type 'T'
# `AnyType` proves nothing about destruction: a fieldwise struct storing a
# bare `Self.T` needs a `Deinitable` bound on `T`.
@fieldwise_init
struct Pair[T: AnyType]:
    var a: Self.T
    var b: Self.T

def main():
    var p = Pair[Int](1, 2)
    print(p.a + p.b)
