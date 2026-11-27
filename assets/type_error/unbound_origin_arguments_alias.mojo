# expect: aliasing values passed mutably to 'a' argument and passed mutably to 'b' argument
# A `Span[Int, _]` parameter's origin is an infer-only binder each call
# binds to its argument's own origin, so two mutable views of one list
# alias, as at the pin.
def f(a: Span[Int, _], b: Span[Int, _]):
    print(len(a) + len(b))


def main():
    var xs = [1, 2, 3]
    f(Span(xs), Span(xs))
