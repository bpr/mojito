# expect: a function type ends a type-parameter bound
# A function type's result takes a later `& Trait`, so the traits of a
# composed bound come before the function type.
def inc(a: Int) -> Int:
    return a + 1


def twice[F: def(Int) -> Int & Copyable](f: F, a: Int) -> Int:
    return f(f(a))


def main():
    print(twice(inc, 5))
