# expect: cannot implicitly convert 'RefBox[origin_of(other)]' value to 'RefBox[origin]'
# A struct's own origin binder is rigid inside its methods: a field typed
# `RefBox[Self.origin]` does not take a box over another origin, as at the
# pin.
@fieldwise_init
struct RefBox[origin: Origin[mut=True]]:
    var value: Pointer[List[Int], Self.origin]

@fieldwise_init
struct Carrier[origin: Origin[mut=True]]:
    var slot: RefBox[Self.origin]

    def restash(mut self, mut other: List[Int]):
        self.slot = RefBox(Pointer(to=other))

def main():
    var xs: List[Int] = [1]
    var ys: List[Int] = [2]
    var c = Carrier(RefBox(Pointer(to=xs)))
    c.restash(ys)
    print(c.slot.value[][0])
