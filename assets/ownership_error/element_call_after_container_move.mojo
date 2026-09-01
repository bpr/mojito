# expect: after it was transferred
# The bare element call reads its receiver like any subscript: calling an
# element of a container that was moved away is a use-after-move of the
# container, caught by ownership analysis before the VM runs. (User-defined
# container: the raw ownership seam checks without the linked stdlib.)
@fieldwise_init
struct Doubler(def(Int) -> Int, Copyable):
    var gain: Int

    def __call__(self, x: Int) -> Int:
        return x * self.gain

@fieldwise_init
struct Container(Copyable):
    var first: Doubler

    def __getitem__(self, index: Int) -> Doubler:
        return self.first.copy()

def consume(var c: Container):
    pass

def main():
    var objs: Container = Container(Doubler(2))
    consume(objs^)
    print(objs[0](3))
