# Runs on both compilers (re-probed 2026-09-08 against the pinned build):
# `unsafe_alloc` imports from `std.memory.alloc`, as upstream, and both print
# `index` / `index` / `42` / `42 2` — the dynamic index of a projected pointer
# subscript passed as a `mut`/`ref` actual is evaluated exactly once.
from std.memory.alloc import unsafe_alloc

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
