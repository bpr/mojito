# A field reference at a `comptime for` index, or over a subject that is
# still a parameter, is a symbolic field step the elaborator closes per
# instance: a generic `total` sums the `Int` fields of any struct, and a
# struct's method writes its own fields through `reflect[Self]`. Every field
# of `Q` is an `Int`, so `types[i]` is `Int` at any index.
@fieldwise_init
struct P(Copyable):
    var x: Int
    var s: String
    var y: Int


@fieldwise_init
struct Q(Copyable):
    var a: Int
    var b: Int
    var c: Int

    def bump(mut self):
        comptime r = reflect[Self]
        comptime for i in range(r.field_count()):
            r.field_ref[i](self) += 1


def total[T: AnyType](x: T) -> Int:
    comptime r = reflect[T]
    comptime types = r.field_types()
    var sum = 0
    comptime for i in range(r.field_count()):
        comptime if types[i] == Int:
            sum += rebind[Int](r.field_ref[i](x))
    return sum


@fieldwise_init
struct Pair[T: Copyable & Deinitable & Writable](Copyable):
    var first: Self.T
    var count: Int

    def bump(mut self):
        comptime r = reflect[Self]
        comptime types = r.field_types()
        comptime for i in range(r.field_count()):
            comptime if types[i] == Int:
                rebind[Int](r.field_ref[i](self)) += 10


def main():
    print(total(P(3, "a", 4)))
    var q = Q(1, 2, 3)
    print(total(q))
    q.bump()
    print(q.a, q.b, q.c)
    var pair = Pair[String]("k", 1)
    pair.bump()
    print(pair.first, pair.count)
