# Explicit imports from the canonical `std.traits`/`std.origin` module homes
# resolve to the compiler builtins and run end-to-end.
from std.traits import Deinitable, Movable, IsTriviallyDeinitable
from std.origin import Origin

struct Res(Movable, Deinitable where False):
    var id: Int

    def __init__(out self, id: Int):
        self.id = id

    def close(deinit self):
        print("closed", self.id)

def borrow[origin: Origin[mut=True]](ref[origin] value: Int) -> ref[origin] Int:
    return value

def main():
    var r = Res(1)
    r^.close()
    var x = 40
    ref y = borrow(x)
    y += 2
    print(x)
    comptime if not IsTriviallyDeinitable[String]:
        print("string deinit nontrivial")
