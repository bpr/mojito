from std.memory import unsafe_alloc

from std.collections.dict import Dict

@fieldwise_init
struct Buffer(Copyable, Movable):
    var data: UnsafePointer[Int, MutUntrackedOrigin]

@fieldwise_init
struct Cursor:
    var calls: Int

    def next(mut self) -> Int:
        print("index")
        self.calls += 1
        return 0

def bump(mut value: Int):
    value += 2

def observe(ref value: Int):
    print(value)

def main() raises:
    var data = unsafe_alloc[Int](1)
    data[0] = 40
    var values = {"a": Buffer(data)}
    var cursor = Cursor(0)
    bump(values["a"].data[cursor.next()])
    observe(values["a"].data[cursor.next()])
    print(values["a"].data[0], cursor.calls)
    data.free()
