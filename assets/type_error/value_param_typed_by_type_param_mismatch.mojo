# expect: value parameter 'w': expected Int, found Bool
# The first value solves `T`; a later value of another type is a mismatch
# reported against its own value parameter.


def pair[T: ImplicitlyCopyable & Writable, //, v: T, w: T]() -> T:
    return w


def main():
    print(pair[1, True]())
