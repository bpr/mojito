# A trait's parameterized value requirement is a value requirement of
# generator type (`__generator_type[idx: Int] Int`), so any member whose
# value converts to that type witnesses it: here an unparameterized member
# bound to another struct's generator. Applied through a bound type
# parameter (`T.get[2]`), the instance's generator value is bound to the
# arguments. A trait default may read the requirement unapplied
# (`Self.get`), as a generator of the same type.


trait HasGet:
    comptime get[idx: Int]: Int
    comptime g: __generator_type[idx: Int] Int = Self.get


struct K:
    comptime k[i: Int]: Int = i * 3


struct W(HasGet):
    comptime get = K.k


struct V(HasGet):
    comptime get[idx: Int]: Int = idx + 1


def read[T: HasGet]() -> Int:
    return T.get[2]


def read_default[T: HasGet]() -> Int:
    return T.g[4]


def main():
    print(read[W]())
    print(read[V]())
    print(read_default[W]())
    print(read_default[V]())
