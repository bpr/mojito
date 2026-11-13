# expect: call to '__getitem__'
# Only runtime-value brackets dispatch the bare element call. A type argument
# in the brackets of an indexable runtime value is a subscript by its
# metatype (`AnyStruct[Int]`), which no `__getitem__` overload accepts; the
# pin says "no matching method in call to '__getitem__'".
@fieldwise_init
struct Doubler(def(Int) -> Int, Copyable):
    var gain: Int

    def __call__(self, x: Int) -> Int:
        return x * self.gain

def main():
    var objs: List[Doubler] = [Doubler(2)]
    print(objs[Int](3))
