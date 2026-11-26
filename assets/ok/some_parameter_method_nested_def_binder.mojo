# A method's `Some[Trait]` parameters are infer-only binders of the method,
# after its declared ones: a nested `def` taking its own `Some[Writable]`
# binds it apart from the method's, in an initializer and in a method with
# a declared parameter of its own.
struct S[T: Writable & Copyable & Deinitable]:
    var v: Self.T

    def __init__(out self, v: Self.T, tag: Some[Writable]):
        self.v = v.copy()
        print(tag)

    def m[k: Int](self, x: Some[Writable]):
        def inner(y: Some[Writable]):
            print(y)

        inner(k)
        print(k, self.v, x)


def main():
    var s = S[Int](1, "t")
    s.m[2](3.5)
