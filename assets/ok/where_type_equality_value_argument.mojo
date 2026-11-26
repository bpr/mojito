# A `where` clause comparing a type parameter with a type over a value
# parameter (`T == Scalar[dtype]`) is proven at the call's arguments, as
# `std.atomic`'s `Atomic` methods declare it: a free function's, a struct's
# static method's over its own `Self.T`, and a custom-self method's.
def plain[T: AnyType]() -> Int where (T == Int):
    return 2


def over_dtype[T: AnyType, dtype: DType]() -> Int where (T == Scalar[dtype]):
    return 1


struct W[T: Deinitable & Movable, *, scope: StaticString = ""]:
    var _value: Self.T

    def __init__(out self, var value: Self.T):
        self._value = value^

    @staticmethod
    def load[
        dtype: DType, //
    ](ptr: ImmPointer[Scalar[dtype], _]) -> Scalar[dtype] where (
        Self.T == Scalar[dtype]
    ):
        return ptr[]

    @__allow_legacy_custom_self_type
    def get[dtype: DType, //](self: W[Scalar[dtype], scope=_]) -> Scalar[dtype]:
        return self._value


def main():
    print(plain[Int]())
    print(over_dtype[Int, DType.int]())
    var x = 4
    print(W[Int].load(Pointer(to=x)))
    var w = W[Int](7)
    print(w.get())
