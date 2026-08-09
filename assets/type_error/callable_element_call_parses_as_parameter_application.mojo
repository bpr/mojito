# expect: parenthesize '(objs[…])(…)' to subscript first
# A recorded subset gap: current Mojo accepts `objs[0](3)` on a List of
# callable structs, but Mojito parses `name[...](...)` over a value base as
# compile-time parameter application and rejects with a parenthesization
# hint. The parenthesized spelling `(objs[0])(3)` subscripts first and
# dispatches through the element (see
# assets/ok/callable_struct_element_invocation.mojo). Real element-call
# dispatch for the bare spelling is future alignment work, recorded in
# roadmap.md.
@fieldwise_init
struct Doubler(def(Int) -> Int, Copyable):
    var gain: Int

    def __call__(self, x: Int) -> Int:
        return x * self.gain

def main():
    var objs: List[Doubler] = [Doubler(2)]
    print(objs[0](3))
