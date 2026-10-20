# A trait requires a parameterized `comptime` value (upstream's generator
# type `__generator_type[idx: Int] Int`); a read through a bound type
# parameter, `T.get[0]`, is answered per instance from the conformer's
# member. Conformance compares the generator signature, so the conformer
# may spell its parameter differently; a keyword binds by the
# requirement's name.


trait HasGet:
    comptime get[idx: Int]: Int


trait HasMore(HasGet):
    comptime size: Int


trait HasSize:
    comptime size_of_elem[T: AnyType]: Int


struct S[*values: Int](HasGet):
    comptime get[idx: Int]: Int = Self.values[idx]


struct A(HasGet, HasSize):
    comptime get[i: Int]: Int = i * 10
    comptime size_of_elem[T: AnyType]: Int = 8


struct B[n: Int](HasGet):
    comptime get[idx: Int]: Int = Self.n + idx


struct L(HasGet):
    comptime get[idx: Int] = 8


struct M(HasMore):
    comptime get[idx: Int]: Int = idx + 1
    comptime size: Int = 3


struct W[T: HasGet]:
    comptime v: Int = Self.T.get[2]

    @staticmethod
    def show():
        print(Self.T.get[3])


def use[T: HasGet]():
    print(T.get[0])
    comptime k = T.get[1]
    print(k)
    print(T.get[0] + T.get[1])


def third[T: HasGet]() -> Int:
    return T.get[3]


def by_keyword[T: HasGet]() -> Int:
    return T.get[idx=5]


def at[T: HasGet, n: Int]() -> Int:
    return T.get[n]


def sum_all[T: HasMore]() -> Int:
    var total = 0
    comptime for i in range(T.size):
        total += T.get[i]
    return total


def elem_size[X: HasSize]() -> Int:
    return X.size_of_elem[Int]


def main():
    use[S[4, 5]]()
    print(third[A](), third[B[100]](), third[L]())
    print(by_keyword[A](), by_keyword[B[1]]())
    print(at[A, 4]())
    print(sum_all[M]())
    print(elem_size[A]())
    W[A].show()
    print(W[B[7]].v)
    comptime n = A.get[1]
    print(n)
