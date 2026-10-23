# expect: call to raising function 'len'
# `len` over a `SizedRaising` value selects upstream's raising overload, so a
# caller that cannot raise rejects it (the pin: "cannot call function that
# may raise in a context that cannot raise").
def count[T: SizedRaising](x: T) -> Int:
    return len(x)


def main():
    pass
