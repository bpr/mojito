# A predicate alias read as a run-time value over a type parameter is the
# `Bool` each instance answers, as at the pin: its `conforms_to` and
# `IsTrivially*` leaves and its type identities (`U == Int`) are decided at
# the instance and the `not`, `and`, and `or` above them fold with them, over
# a function's binder and over the enclosing struct's (`C[Self.T]`), in a
# run-time read and a `comptime if`.
from std.traits import IsTriviallyCopyable

comptime C[U: AnyType] = conforms_to(U, Copyable)
comptime NotCopy[U: AnyType] = not conforms_to(U, Copyable)
comptime Either[U: AnyType] = conforms_to(U, Copyable) or conforms_to(
    U, Defaultable
)
comptime IsSmallCopy[U: AnyType] = conforms_to(U, Copyable) and IsTriviallyCopyable[U]
comptime NotTrivial[U: AnyType] = not IsTriviallyCopyable[U]
comptime IsInt[U: AnyType] = U == Int
comptime CopyNotInt[U: AnyType] = U != Int and conforms_to(U, Copyable)


struct NC(Movable):
    var x: Int


def not_copy[T: AnyType]() -> Bool:
    return NotCopy[T]


def either[T: AnyType]() -> Bool:
    return Either[T]


def small_copy[T: AnyType]() -> Bool:
    return IsSmallCopy[T]


def not_trivial[T: AnyType]() -> Bool:
    return NotTrivial[T]


def is_int[T: AnyType]() -> Bool:
    return IsInt[T]


def copy_not_int[T: AnyType]() -> Bool:
    comptime if CopyNotInt[T]:
        return True
    return False


struct Box[T: AnyType]:
    @staticmethod
    def c() -> Bool:
        return C[Self.T]

    @staticmethod
    def d() -> Bool:
        comptime if C[Self.T]:
            return True
        return False

    @staticmethod
    def e[V: AnyType]() -> Bool:
        return C[Self.T] and C[V]


def main():
    print(not_copy[Int](), not_copy[NC](), not_copy[String]())
    print(either[Int](), either[NC](), either[String]())
    print(small_copy[Int](), small_copy[NC](), small_copy[String]())
    print(not_trivial[Int](), not_trivial[NC](), not_trivial[String]())
    print(is_int[Int](), is_int[NC](), is_int[String]())
    print(copy_not_int[Int](), copy_not_int[NC](), copy_not_int[String]())
    print(Box[Int].c(), Box[NC].c(), Box[Int].d(), Box[NC].d())
    print(Box[Int].e[Int](), Box[Int].e[NC](), Box[NC].e[Int]())
