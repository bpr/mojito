# expect: aliasing values passed mutably to 'items' argument and passed mutably to 'default' argument in 'pick' call
# An origin a generic function receives only through a type argument
# (`pick`'s `T` bound to `Span[Int, origin_of(xs)]`) counts in argument
# exclusivity, whatever the parameters' conventions: two arguments carrying
# the mutable `xs` alias, as at the pin.
def pick[T: Copyable](items: List[T], default: T) -> T:
    if len(items) > 0:
        return items[0].copy()
    return default.copy()


def main():
    var xs: List[Int] = [1, 2, 3]
    var l = List[Span[Int, origin_of(xs)]]()
    l.append(Span(xs))
    print(pick(l, Span(xs))[2])
