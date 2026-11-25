# A `Span` element of a read `List[String]` converted implicitly to a
# `StringSlice` argument while the span stays live. The conversion's
# `ref [Self.origin]` takes the element, which the span's loan does not
# exclude, so it runs, as at the pin: `a`, `2`.
def show(s: StringSlice):
    print(s)


def f(items: List[String]):
    var span = Span(items)
    show(span[0])
    print(len(span))


def main():
    f(["a", "b"])
