# expect: cannot materialize comptime value of type 'Array[
# Indexing a compile-time list of field names at runtime materializes the
# whole list, which is not implicitly copyable: the element crosses through
# `materialize[names[i]]()` instead.
@fieldwise_init
struct Point:
    var x: Int
    var y: Int


def show[T: AnyType]():
    comptime names = reflect[T].field_names()
    comptime for i in range(len(names)):
        print(names[i])


def main():
    show[Point]()
