# expect: aliasing values passed mutably to 'xs' argument and passed mutably to 'b' argument in 'f' call
# A `mut` argument's own place and another argument's carried origin over the
# same storage may not reach one call, as at the pin.
@fieldwise_init
struct RefBox[origin: Origin[mut=True]]:
    var value: Pointer[List[Int], Self.origin]

def f[o: Origin[mut=True]](mut xs: List[Int], b: RefBox[o]):
    xs.append(b.value[][0])

def main():
    var xs: List[Int] = [9]
    f(xs, RefBox(Pointer(to=xs)))
    print(len(xs))
