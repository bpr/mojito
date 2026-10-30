# expect: type mismatch for value parameter 'c'
# A display whose elements are not the `Span` parameter's element type
# converts through no `Span` constructor.


def f[c: Span[Int, _]]():
    print(len(c))


def main():
    f[[String("a"), String("b")]]()
