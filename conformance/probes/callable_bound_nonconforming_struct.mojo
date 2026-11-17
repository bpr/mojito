# Pin gap probe (Mojo 1.3.0.dev2026111316): a struct that declares no
# callable conformance, passed at a `def(...)` bound. The pin rejects it with
# "value passed to 'f' cannot be converted from 'S' to 'F', argument type 'S'
# does not conform to trait 'def(Int) -> Int'"; Mojito says "type 'S' for
# parameter 'F' does not conform to trait 'def(Int) -> Int': requires one
# monomorphic callable type or nominal callable conformance".
# docs/roadmap.md R2292.


struct S:
    var v: Int

    def __init__(out self, v: Int):
        self.v = v


def apply[F: def(Int) -> Int](f: F, x: Int) -> Int:
    return f(x)


def main():
    print(apply(S(1), 2))
