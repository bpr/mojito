# A generic `def` constructs a reflected field type under a `comptime for`
# its template serves: `types[i]()` and `FT()` over an alias of the element
# build the field type's default, and `x: types[i]` and `x: FT` annotate a
# local with it, once a `conforms_to` arm proves the construction.
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
    var first: Int
    var second: Unit
    var third: Float64
    var fourth: Zero
    var fifth: Bool


def construct[T: AnyType]():
    comptime types = reflect[T].field_types()
    comptime for i in range(reflect[T].field_count()):
        comptime if conforms_to(
            types[i], Defaultable & Deinitable & Writable & Copyable
        ):
            var x = types[i]()
            var y: types[i] = x^
            print("by index:", y)
        else:
            print("no default")


def construct_alias[T: AnyType]():
    comptime types = reflect[T].field_types()
    comptime for i in range(reflect[T].field_count()):
        comptime FT = types[i]
        comptime if conforms_to(FT, Defaultable & Deinitable & Writable & Copyable):
            var x: FT = FT()
            print("by alias:", x)


def main():
    construct[Holder]()
    construct_alias[Holder]()
