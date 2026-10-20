# A trait's compile-time value member read through a bound type parameter
# folds per instance in a compile-time position of a template: a `comptime
# if` condition, a lane width, and a `comptime for` range bound in a method
# with a binder of its own.
trait HasK:
    comptime K: Int


struct Q(HasK):
    comptime K = 3


def g[T: HasK]():
    comptime if T.K > 2:
        print("big")
    var x = SIMD[DType.int32, T.K - 1](1)
    print(x)


struct S:
    def __init__(out self):
        pass

    def m[T: HasK](self):
        comptime for i in range(T.K):
            print(i)


def main():
    g[Q]()
    S().m[Q]()
