# `None` in an annotation's bracket is the `None` value, as at the pin, so
# `List`'s type parameter rejects it.
# expect: expected a type, found a value


def main():
    var x: List[None] = []
    print(len(x))
