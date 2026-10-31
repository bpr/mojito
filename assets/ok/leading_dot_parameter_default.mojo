# A leading-dot parameter default names a member of the parameter's declared
# type, as a leading-dot bracket argument does.


@fieldwise_init
struct Mode(Equatable, ImplicitlyCopyable):
    var code: Int

    comptime fast = Mode(1)
    comptime slow = Mode(2)

    def __eq__(self, other: Self) -> Bool:
        return self.code == other.code


struct Lanes[n: Int, *, dt: DType = .int64](ImplicitlyCopyable):
    var x: Scalar[Self.dt]

    def __init__(out self):
        self.x = Scalar[Self.dt](Self.n)


def name[d: DType = .int64]() -> String:
    return String(d)


def speed[m: Mode = .slow]() -> Int:
    return m.code


def main():
    print(name())
    print(name[.int8]())
    var a = Lanes[3]()
    var b = Lanes[4, dt=.int32]()
    print(a.x, a.dt, b.x, b.dt)
    print(speed(), speed[.fast]())
