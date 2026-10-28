# expect: Undefined variable 'x'
# A signature's `type_of` reads only the parameters declared before it:
# `x` is not yet in scope where `y`'s type is resolved.


def f(y: type_of(x), x: Int) -> Int:
    return x + y


def main():
    print(f(1, 2))
