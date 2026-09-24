# expect: type mismatch for variable 'x': expected Int, found StringLiteral
# A body reading `reflect[T]` is validated from its template like any other:
# `reflect[T].field_count()` is a compile-time `Int` while `T` is symbolic,
# so the untaken arm's type error is reported with nothing calling
# `field_count`.
def field_count[T: AnyType]() -> Int:
    comptime if reflect[T].field_count() == 2:
        return 2
    else:
        var x: Int = "oops"
        return 0


def main():
    print("never instantiated")
