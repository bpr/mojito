# expect: aliasing values passed immutably to 'v' argument and constructed as a result in 'rebuild' call
# A contiguous `List` slice is a `Span` over the list's owned elements, so a
# call reading the slice cannot construct its result into the list itself.
def rebuild(v: Span[Int, _]) -> List[Int]:
    return [v[0], v[0]]

def main():
    var xs: List[Int] = [1, 2]
    xs = rebuild(xs[0:1])
    print(xs[1])
