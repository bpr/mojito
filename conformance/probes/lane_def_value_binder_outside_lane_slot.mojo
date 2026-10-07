# Pin gap probe (Mojo 1.2.0.dev2026092105): a `DType`-keyed `def` whose `Int`
# value binder a runtime parameter names outside a lane slot (`Box[n]`). The
# pin prints `3` and `2`; Mojito rejects the inferred call, "generic 'h'
# requires compile-time parameter 'dt'", since the template does not serve
# the body and the cloner's `DType` stub cannot bind it. Roadmap R477.
struct Box[n: Int](Copyable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v


def h[dt: DType, n: Int](a: Box[n], b: Scalar[dt]) -> Int:
    return n


def main():
    print(h(Box[3](0), Int32(1)))
    print(h[DType.int8, 2](Box[2](0), Int8(1)))
