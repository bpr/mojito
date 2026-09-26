# A method overloaded on a `Scalar[dt]` pattern and a bare trait-bound
# parameter: an argument with a lane selects the `Scalar[dt]` pattern, as a
# free call does, and an argument without one selects the bare `T`.


struct Classifier:
    def __init__(out self):
        pass

    def kind[dt: DType](self, a: Scalar[dt]) -> Int:
        return 2

    def kind[T: Copyable](self, a: T) -> Int:
        return 3


def main():
    var c = Classifier()
    var x: Float64 = 1.5
    print(c.kind(x))
    var y: Int32 = 4
    print(c.kind(y))
    print(c.kind(7))
    print(c.kind(String("s")))
