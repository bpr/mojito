# A parameter default applies a callable over an earlier `DType` binder, or
# over its struct's binder in a static method, at the call's arguments.
def widened[dt: DType]() -> DType:
    return DType.int32 if dt == DType.float32 else dt


def twice(n: Int) -> Int:
    return 2 * n


def f[dtype: DType, _dtype: DType = widened[dtype]()]():
    print(_dtype)


def g[n: Int, m: Int = twice(n)]():
    print(m)


struct S[dtype: DType]:
    @staticmethod
    def h[_dtype: DType = widened[Self.dtype]()]():
        print(_dtype)


struct T[n: Int]:
    @staticmethod
    def next[m: Int = Self.n + 1]():
        print(m)

    @staticmethod
    def doubled[m: Int = twice(Self.n)]():
        print(m)


def main():
    f[DType.float32]()
    f[DType.int8]()
    g[3]()
    S[DType.float32].h()
    T[3].next()
    T[3].doubled()
