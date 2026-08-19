# The parenthesized spelling for calling a callable-struct List element:
# parenthesizing forces the subscript to parse first, and the Invoke channel
# dispatches the element's `__call__`. The bare `objs[0](3)` spelling now
# dispatches identically through the element-call re-dispatch (see
# assets/ok/callable_element_call_dispatch.mojo for both spellings).
@fieldwise_init
struct Doubler(def(Int) -> Int, Copyable):
    var gain: Int

    def __call__(self, x: Int) -> Int:
        return x * self.gain

def main():
    var objs: List[Doubler] = [Doubler(2)]
    print((objs[0])(3))
