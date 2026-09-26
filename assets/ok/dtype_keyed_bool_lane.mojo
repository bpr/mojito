# A `Bool` argument binds a lone `Scalar[dt]` pattern's lane at
# `DType.bool`: the `Bool` converts into `Scalar[DType.bool]`, as the pinned
# Mojo does, for a free `def` and for a method alike.


struct Lanes:
    def __init__(out self):
        pass

    def kind[dt: DType](self, a: Scalar[dt]) -> String:
        return String(dt)


def kind[dt: DType](a: Scalar[dt]) -> String:
    return String(dt)


def same[dt: DType](a: Scalar[dt]) -> Scalar[dt]:
    return a


def main():
    print(kind(True))
    var b = False
    print(kind(b))
    print(Lanes().kind(True))
    print(same(b))
    print(kind(Float32(1.5)))
