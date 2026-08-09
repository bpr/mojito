# expect: 'objs' has type List and is not callable
# A real parity divergence: current Mojo accepts `objs[0](3)` on a List of
# callable structs, but Mojito parses `name[...](...)` over a value base as
# compile-time parameter application, so the checker sees a generic
# application of the non-callable List binding. The parenthesized spelling
# `(objs[0])(3)` subscripts first and dispatches through the element (see
# assets/ok/callable_struct_element_invocation.mojo). Disambiguating the
# bare spelling on lowercase value bases is a parser-altitude question,
# recorded in docs/features.md.
@fieldwise_init
struct Doubler(def(Int) -> Int, Copyable):
    var gain: Int

    def __call__(self, x: Int) -> Int:
        return x * self.gain

def main():
    var objs: List[Doubler] = [Doubler(2)]
    print(objs[0](3))
