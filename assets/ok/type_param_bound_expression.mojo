# A type-parameter bound is one expression, converted in the type context:
# a parenthesized function type composes with a trait, a function type may
# lead a composition, `None` types a value parameter, and a parenthesized
# composition is its traits.
def inc(a: Int) -> Int:
    return a + 1

def report(a: Int):
    print("report", a)

def twice[F: (def(Int) -> Int) & ImplicitlyCopyable](f: F, a: Int) -> Int:
    return f(f(a))

def call_with[F: def(Int) & Copyable](f: F):
    f(3)

def show[x: None]():
    print("none")

def pick[T: (Copyable & Movable)](t: T) -> T:
    return t.copy()

def main():
    print(twice(inc, 5))
    call_with(report)
    show[None]()
    print(pick(4))
