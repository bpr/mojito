# A reflection query over a template-served body's parameter reaches MIR as
# a parameter constant, which the elaborator answers per instance from the
# struct the parameter binds: a count, an index, `is_struct()`, the length
# of either list, an element of `field_names()`, a runtime `if`, a
# `comptime if`, a bound handle or count, and `reflect[Self.T]` or
# `reflect[Self]` in a generic struct's method.
@fieldwise_init
struct Pair(Copyable):
    var left: Int
    var right: Int


@fieldwise_init
struct Single(Copyable):
    var only: Int


@fieldwise_init
struct Box[T: Copyable & Deinitable](Copyable):
    var item: Self.T
    var tag: Int

    def inner_count(self) -> Int:
        return reflect[Self.T].field_count()

    def own_count(self) -> Int:
        return reflect[Self].field_count()


def count[T: AnyType]() -> Int:
    return reflect[T].field_count()


def is_struct[T: AnyType]() -> Bool:
    return reflect[T].is_struct()


def right_index[T: AnyType]() -> Int:
    return reflect[T].field_index["right"]()


def name_count[T: AnyType]() -> Int:
    return len(reflect[T].field_names())


def show_second[T: AnyType]():
    print(reflect[T].field_names()[1])


def runtime_branch[T: AnyType]() -> Int:
    if reflect[T].field_count() == 2:
        return 1
    return 0


def comptime_branch[T: AnyType]() -> Int:
    comptime if reflect[T].field_count() == 2:
        return 2
    else:
        return 0


def bound_count[T: AnyType]() -> Int:
    comptime n = reflect[T].field_count()
    return n + 10


def bound_handle[T: AnyType]() -> Int:
    comptime r = reflect[T]
    comptime fields = r.field_count()
    return fields * 100 + r.field_index["right"]()


def main():
    print(count[Pair](), count[Single]())
    print(is_struct[Pair]())
    print(right_index[Pair]())
    print(name_count[Pair]())
    show_second[Pair]()
    print(runtime_branch[Pair](), runtime_branch[Single]())
    print(comptime_branch[Pair](), comptime_branch[Single]())
    print(bound_count[Single]())
    print(bound_handle[Pair]())
    var b = Box(Pair(1, 2), 3)
    print(b.inner_count(), b.own_count())
