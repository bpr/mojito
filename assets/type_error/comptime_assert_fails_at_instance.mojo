# expect: function instantiation of `f` failed: constraint failed: need Copyable
# A `comptime assert` that does not hold at a reached instance fails that
# instance with its message, as at the pin.

def f[T: AnyType]():
    comptime assert conforms_to(T, Copyable), "need Copyable"
    print("ok")


struct NC:
    pass


def main():
    f[NC]()
