# A local typed by a subscript of a `TypeList` construction over a pack
# keeps the element symbolic in the generic body, so a literal does not
# convert to it, as at the pin.
# expect: type mismatch for variable 'x': expected Ts[1], found Int
def f[*Ts: AnyType]():
    var x: TypeList[Trait=AnyType, Ts.values]()[1] = 3
    print("ok")


def main():
    f[Bool, Int]()
