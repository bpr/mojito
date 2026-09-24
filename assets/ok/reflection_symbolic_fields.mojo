# A body reading `reflect[T]` over a symbolic `T` is validated once from its
# template and elaborated per instance: `field_count()`, `field_index[..]()`,
# and `len(field_names())` are compile-time `Int`s, `is_struct()` a `Bool`, a
# field type under the loop index is opaque until a `conforms_to` arm proves
# a trait of it, and `types[i] == Int` selects an arm without narrowing.
struct Zero(Copyable, Defaultable, Writable):
    var v: Int

    def __init__(out self):
        self.v = 0

    def write_to(self, mut writer: Some[Writer]):
        writer.write("Zero")


struct Unit(Copyable, Writable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def write_to(self, mut writer: Some[Writer]):
        writer.write("Unit(", self.v, ")")


@fieldwise_init
struct Holder(Copyable):
    var first: Zero
    var second: Unit
    var third: Int


def describe[T: AnyType]():
    comptime r = reflect[T]
    comptime names = r.field_names()
    comptime types = r.field_types()
    comptime count: Int = len(names)
    comptime second: Int = r.field_index["second"]()
    comptime if r.field_count() == 3:
        print("three fields, second at", second, "of", count)
    else:
        print("not three fields")
    comptime for i in range(r.field_count()):
        comptime FT = types[i]
        comptime if types[i] == Int:
            print("an Int field")
        elif conforms_to(FT, Defaultable & Deinitable & Writable):
            print("a default:", FT())
        else:
            print("a field of another type")
    comptime if r.is_struct():
        print("a struct")


def main():
    describe[Holder]()
