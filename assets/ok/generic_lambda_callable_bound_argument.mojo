# A capture-free generic lambda or nested generic `def` passed at a generic
# callable bound (`def[n: Int]() -> Int`) binds as its generator's symbol,
# applied to the enclosing binders with its own left open, as at the pin.
# Two nested generic `def`s of one signature, spelled with different binder
# names in different bodies, inflate to one type.
def apply[F: ImplicitlyCopyable & RegisterPassable & def[n: Int]() -> Int, //](f: F) -> Int:
    return f[3]() + f[4]()


def outer[k: Int]() -> Int:
    return apply(lambda [n: Int]() -> Int: n + k)


def tripled() -> Int:
    def body[w: Int]() -> Int:
        return w * 3

    return apply(body)


def offset[k: Int]() -> Int:
    def body[n: Int]() -> Int:
        return n + k

    return apply(body)


def main():
    print(outer[10]())
    print(apply(lambda [n: Int]() -> Int: n * 2))
    print(tripled())
    print(offset[100]())
