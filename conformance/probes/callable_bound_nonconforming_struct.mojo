# Pin probe (Mojo 1.3.0.dev2026111316): a struct that declares no
# callable conformance, passed at a `def(...)` bound. The pin rejects it with
# "value passed to 'f' cannot be converted from 'S' to 'F', argument type 'S'
# does not conform to trait 'def(Int) -> Int'", and Mojito
# says the same (assets/type_error/callable_bound_nonconforming_struct.mojo).


struct S:
    var v: Int

    def __init__(out self, v: Int):
        self.v = v


def apply[F: def(Int) -> Int](f: F, x: Int) -> Int:
    return f(x)


def main():
    print(apply(S(1), 2))
