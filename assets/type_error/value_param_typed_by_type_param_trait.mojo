# expect: does not conform to trait 'Intable'
# The type a value solves for `T` must satisfy `T`'s bounds.


def f[T: Intable & ImplicitlyCopyable, //, v: T]() -> T:
    return v


def main():
    _ = f["a"]()
