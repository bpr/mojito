# A reflected field-name list materialized whole over a type parameter,
# `materialize[names]()`, is the `Array` of the names sized by the field
# count, constructed per instance from the template: in a generic `def` and
# in a generic struct's method, copied, indexed, and iterated. A query called
# at run time (`reflect[T].field_names()`) is the same `Array`.
@fieldwise_init
struct Point:
    var x: Int
    var y: Int


@fieldwise_init
struct Triple[T: Copyable & Deinitable]:
    var a: Self.T
    var b: Int
    var c: String


@fieldwise_init
struct Holder[T: AnyType](Copyable):
    var k: Int

    def names(self) -> Int:
        comptime names = reflect[Self.T].field_names()
        var all = materialize[names]()
        for name in all:
            print(name)
        return len(all) + self.k


def show[T: AnyType]() -> Int:
    comptime names = reflect[T].field_names()
    var all = materialize[names]()
    var copy = all.copy()
    print(len(copy), copy[len(copy) - 1])
    return len(all)


def direct[T: AnyType]():
    var names = reflect[T].field_names()
    print(len(names), names[0])


def main():
    print(show[Point](), show[Triple[Int]]())
    direct[Triple[Int]]()
    var names = reflect[Point].field_names()
    print(names[1])
    print(Holder[Point](10).names())
    print(Holder[Triple[Float64]](20).names())
