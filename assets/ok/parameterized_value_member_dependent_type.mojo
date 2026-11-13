# A parameterized `comptime` value's declared type reads the member's own
# parameters, in a struct and in a trait requirement read through a bound;
# a conformer's binders align with the requirement's by position.


trait HasGet:
    comptime get[n: Int]: SIMD[DType.int32, n]


trait MoreGet(HasGet):
    comptime get[n: Int]: SIMD[DType.int32, n]


trait HasAny:
    comptime pick[T: AnyType]: T


struct A:
    comptime get[n: Int]: SIMD[DType.int32, n] = SIMD[DType.int32, n](7)


struct B(HasGet):
    comptime get[k: Int]: SIMD[DType.int32, k] = SIMD[DType.int32, k](k)


struct C(MoreGet):
    comptime get[n: Int]: SIMD[DType.int32, n] = SIMD[DType.int32, n](5)


def show[T: HasGet]():
    print(T.get[4])


def show_more[T: MoreGet]():
    print(T.get[2])


def main():
    print(A.get[2])
    show[B]()
    show_more[C]()
