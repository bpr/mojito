# A field name under a `comptime for` index crosses to runtime explicitly:
# `materialize[names[i]]()` copies the one element, a `comptime` binding of it
# is an implicitly copyable string, and a query spelled in place is a runtime
# call. A bare `names[i]` would materialize the whole name list, which is not
# implicitly copyable.
@fieldwise_init
struct Point:
    var x: Int
    var y: Int


def show[T: AnyType]():
    comptime names = reflect[T].field_names()
    comptime for i in range(len(names)):
        print(materialize[names[i]]())
        comptime name = names[i]
        print(name, reflect[T].field_names()[i])
    var all = materialize[names]()
    print(len(all), all[0])


def main():
    show[Point]()
