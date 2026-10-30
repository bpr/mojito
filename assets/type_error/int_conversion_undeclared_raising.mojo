# expect: does not conform to trait 'Intable'
# A struct whose `__int__` raises converts through `Int(x)` only by
# declaring `IntableRaising`: with neither trait no `SIMD.__init__`
# overload matches, as at the pin.
struct R:
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __int__(self) raises -> Int:
        return self.v


def main() raises:
    print(Int(R(3)))
