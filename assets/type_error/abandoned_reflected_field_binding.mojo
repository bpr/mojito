# A reflected field type is opaque in the generic body: a `conforms_to` arm
# proving `Defaultable & Writable` lets the body build one, but not destroy
# it, so the binding is abandoned until `Deinitable` is proved too.
# expect: 'v' abandoned without being explicitly destroyed: unhandled explicitly destroyed type 'Defaultable'
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
        comptime FT = types[i]
        comptime if conforms_to(FT, Defaultable & Writable):
            var v: FT = FT()
            print(v)


def main():
    construct[Holder]()
