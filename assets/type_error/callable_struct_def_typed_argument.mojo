# A struct conforming to a `def(...)` contract is called directly, but it is
# no value of that function type: current Mojo will not convert it where a
# `def(...)` parameter is expected.
# expect: expected def(Int) -> Int, found Scale
@fieldwise_init
struct Scale(def(Int) -> Int):
    var factor: Int

    def __call__(self, value: Int) -> Int:
        return value * self.factor

def keep(callback: def(Int) -> Int):
    pass

def main():
    print(Scale(3)(14))
    keep(Scale(3))
