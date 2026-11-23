# expect: 'AnyStruct[Int]' does not implement the '__and__' method
# A function type's result is one whole expression, so a later `& Trait` is
# part of it, and evaluated in the type context it is no type: the traits of
# a composed bound come before the function type.
def inc(a: Int) -> Int:
    return a + 1


def twice[F: def(Int) -> Int & Copyable](f: F, a: Int) -> Int:
    return f(f(a))


def main():
    print(twice(inc, 5))
