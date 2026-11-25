# A parameter typed by a conditional over a value binder conforms, inside
# its generic body, to every trait both arms conform to: it copies, prints,
# returns as `type_of(x)`, binds a `Writable` parameter, and answers
# `conforms_to` before the call selects an arm.
struct Tag(Copyable, Writable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def write_to(self, mut writer: Some[Writer]):
        writer.write("Tag(", self.v, ")")


def show[T: Writable](t: T):
    print(t)


def copied[n: Int](x: Int if n > 0 else Float64) -> type_of(x):
    var y = x
    return y


def shown[n: Int](x: Int if n > 0 else Tag):
    print(x)
    show(x)
    comptime if conforms_to(type_of(x), Writable):
        print("writable")


def main():
    print(copied[1](3))
    print(copied[0](2.5))
    shown[1](4)
    shown[0](Tag(5))
