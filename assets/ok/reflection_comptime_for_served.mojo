# A `comptime for` bounded by a reflection count, or iterating a reflected
# field-name list, over a type that is still a parameter is served by the
# template: the bound and the sequence are queries the elaborator answers
# per instance. A field name crosses to runtime as one element
# (`materialize[names[i]]()`, a `comptime` binding of it), and a field type
# decides a `comptime if` by identity or by conformance.
@fieldwise_init
struct Point:
    var x: Int
    var y: String


def count[T: AnyType]():
    comptime for i in range(reflect[T].field_count()):
        print(i)


def bound[T: AnyType]():
    comptime r = reflect[T]
    comptime for i in range(r.field_count()):
        print(i + 10)


def names[T: AnyType]():
    comptime for name in reflect[T].field_names():
        print(name)


def indexed[T: AnyType]():
    comptime names = reflect[T].field_names()
    comptime for i in range(len(names)):
        comptime name = names[i]
        print(name, materialize[names[i]]())


def kinds[T: AnyType]():
    comptime r = reflect[T]
    comptime types = r.field_types()
    comptime if r.field_count() == 2:
        print("two fields")
    comptime for i in range(r.field_count()):
        comptime if types[i] == Int:
            print("an Int field")
        elif conforms_to(types[i], Writable & Copyable):
            print("a writable field")
        else:
            print("another field")


def main():
    count[Point]()
    bound[Point]()
    names[Point]()
    indexed[Point]()
    kinds[Point]()
    comptime for name in reflect[Point].field_names():
        print(name)
