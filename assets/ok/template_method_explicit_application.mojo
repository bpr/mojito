# A per-instantiation method clone inherits its checked template's facts when
# its body calls a generic module function with explicit type arguments
# naming the struct parameter (`docs/notes/instantiation-from-template.md`):
# `unsafe_alloc[Self.T](n)` records its application with the template's
# arguments, which each instance substitutes and realizes as the clone the
# elaborator retargeted the call to. The result is stored to a field, bound
# to a `var` local, and read back from that local, and an untracked pointer
# field hands itself to a constructor's read parameter, as the owned
# `List.__iter__` does.
from std.memory.alloc import unsafe_alloc


struct Handle[T: AnyType](Movable):
    var data: UnsafePointer[Self.T, MutUntrackedOrigin]
    var cap: Int

    def __init__(out self, data: UnsafePointer[Self.T, MutUntrackedOrigin], cap: Int):
        self.data = data
        self.cap = cap

    def release(deinit self) -> Int:
        self.data.unsafe_free()
        return self.cap


struct Pool[T: Copyable & Deinitable](Movable):
    var data: UnsafePointer[Self.T, MutUntrackedOrigin]
    var cap: Int

    def __init__(out self, cap: Int):
        self.data = unsafe_alloc[Self.T](cap)
        self.cap = cap

    def regrow(mut self, cap: Int):
        var fresh = unsafe_alloc[Self.T](cap)
        self.data.unsafe_free()
        self.data = fresh
        self.cap = cap

    def take(var self) -> Handle[Self.T]:
        var result = Handle[Self.T](self.data, self.cap)
        self.data = unsafe_alloc[Self.T](0)
        self.cap = 0
        return result^

    def __deinit__(deinit self):
        self.data.unsafe_free()


def main():
    var ints = Pool[Int](2)
    ints.regrow(8)
    print(ints.cap)
    var handle = ints^.take()
    print(handle^.release())
    var strings = Pool[String](1)
    strings.regrow(3)
    var other = strings^.take()
    print(other^.release())
