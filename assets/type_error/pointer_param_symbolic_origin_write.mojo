# expect: write through a Pointer whose origin mutability is not known here
# A free function's `Pointer[T, o]` parameter over its own `Origin[mut=m]`
# binder is symbolic in the body, exactly like a struct's field.
def poke[m: Bool, //, o: Origin[mut=m]](p: Pointer[List[Int], o]):
    p[][0] = 9

def main():
    var xs = List[Int]()
    xs.append(7)
    poke(Pointer(to=xs))
    print(xs[0])
