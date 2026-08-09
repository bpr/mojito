# The accepted spelling for calling a callable-struct List element:
# parenthesizing forces the subscript to parse first, and the Invoke channel
# dispatches the element's `__call__`. The bare `objs[0](3)` spelling parses
# as compile-time parameter application and rejects (see
# assets/type_error/callable_element_call_parses_as_parameter_application.mojo).
@fieldwise_init
struct Doubler(def(Int) -> Int, Copyable):
    var gain: Int

    def __call__(self, x: Int) -> Int:
        return x * self.gain

def main():
    var objs: List[Doubler] = [Doubler(2)]
    print((objs[0])(3))
