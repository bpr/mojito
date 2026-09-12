# expect: which names no in-scope Origin parameter
# A struct's explicit origin slot resolves only in-scope Origin binders (the
# enclosing struct's, a method's, or the function's own) and the builtin
# origins. A plain type parameter is in scope but names no origin, so the
# application is rejected.
@fieldwise_init
struct RefBox[origin: Origin[mut=True]]:
    var value: ref[origin] List[Int]

def make[T: Copyable](box: RefBox[T]):
    pass

def main():
    print(1)
