# A `comptime` binding over another local one (`names` over the handle
# `r = reflect[T]`) reads the binders the handle's subject names, so a
# lifted application over it (`range(names.length)`) is over `T` alone,
# beside a callable parameter it does not read.
from std.reflection import reflect


def count_fields[T: AnyType](this: T, mut writer: Some[Writer]):
    comptime r = reflect[T]
    comptime names = r.field_names()
    comptime for i in range(names.length):
        writer.write_string("f")


def each_field[
    T: AnyType,
    W: Writer,
    //,
    f: def[FieldType: Writable](field: FieldType, mut writer: W) thin,
](this: T, mut writer: W):
    comptime r = reflect[T]
    comptime names = r.field_names()
    comptime n = names.length
    comptime for i in range(n):
        f(i, writer)


def write_one[FieldType: Writable](field: FieldType, mut writer: String):
    field.write_to(writer)


@fieldwise_init
struct Point(Writable):
    var x: Int
    var y: String


def main():
    var s = String()
    count_fields(Point(1, "a"), s)
    print(s)
    var t = String()
    each_field[f=write_one](Point(1, "a"), t)
    print(t)
