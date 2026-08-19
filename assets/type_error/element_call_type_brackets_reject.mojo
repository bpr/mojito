# expect: only for runtime index arguments
# Only runtime-value brackets dispatch the bare element call. A type argument
# in the brackets of an indexable runtime value is neither a subscript nor
# parameter application (a value takes no compile-time parameters), so the
# shape is rejected with the residual diagnostic.
@fieldwise_init
struct Doubler(def(Int) -> Int, Copyable):
    var gain: Int

    def __call__(self, x: Int) -> Int:
        return x * self.gain

def main():
    var objs: List[Doubler] = [Doubler(2)]
    print(objs[Int](3))
