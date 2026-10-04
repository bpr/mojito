# A `comptime if` whose condition asks a layout query (`size_of[Self.T]()` in
# a generic struct's method, `size_of[Padded]()` in a plain one, `size_of[Int]()`
# in `main`) is decided by the elaborator under the compilation's target, as
# at the pin.
from std.sys import size_of

@fieldwise_init
struct Padded:
    var flag: Bool
    var value: Int

struct Holder[T: Copyable & Deinitable & Defaultable]:
    var value: Self.T

    def __init__(out self):
        self.value = Self.T()

    def width(self) -> Int:
        comptime if size_of[Self.T]() == 1:
            return 1
        elif size_of[Self.T]() == 4 and size_of[Int]() == 8:
            return 4
        else:
            return size_of[Self.T]()

struct Plain:
    var x: Int

    def __init__(out self):
        self.x = 3

    def get(self) -> Int:
        comptime if size_of[Padded]() == 16:
            return self.x
        else:
            return 0

def main():
    print(Holder[Int8]().width())
    print(Holder[Int32]().width())
    print(Holder[Int]().width())
    print(Plain().get())
    comptime if size_of[Int]() == 8:
        print("64-bit")
    else:
        print("32-bit")
# stdout: 1
# stdout: 4
# stdout: 8
# stdout: 3
# stdout: 64-bit
