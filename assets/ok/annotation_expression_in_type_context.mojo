# An annotation is one whole expression evaluated in its type context, as
# upstream evaluates it: a conditional of types is a type wherever an
# annotation stands.
comptime H = __generator_type[T: AnyType] Int if True else Int


def describe(f: def(Int) -> Int if True else Int) -> String:
    return "function"


def double(x: Int if True else Int) -> Int if True else Int:
    return x * 2


def shifted[n: Int if True else Int]() -> Int:
    return n + 10


def main():
    var y: Int if True else Int = double(3)
    print(y)
    print(shifted[5]())
