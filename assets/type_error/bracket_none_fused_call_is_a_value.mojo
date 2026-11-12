# `None` in a called function's bracket is the `None` value, as at the pin,
# so a type parameter rejects it.
# expect: expected a type, found a value


def show[T: AnyType]():
    print("show")


def main():
    show[None]()
