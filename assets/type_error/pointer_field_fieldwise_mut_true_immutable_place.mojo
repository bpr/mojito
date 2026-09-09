# expect: type mismatch for field
# A `@fieldwise_init` struct whose origin binder is spelled `mut=True`
# still rejects a pointer to an immutable place (a read parameter): only a
# parametric `mut=m` binder takes its mutability from the place.
@fieldwise_init
struct P[o: Origin[mut=True]]:
    var src: Pointer[List[Int], Self.o]

    def get(self) -> Int:
        return self.src[][0]

def show(xs: List[Int]):
    var p = P(Pointer(to=xs))
    print(p.get())

def main():
    var xs = List[Int]()
    xs.append(7)
    show(xs)
