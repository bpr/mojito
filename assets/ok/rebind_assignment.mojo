# `rebind[Dest](place) = value` writes through the rebound place: an operand
# whose declared type is not `TrivialRegisterPassable` selects upstream's
# `ref[src]` overload, so the target is the operand itself and the value it
# replaces is destroyed at the assignment.
struct Tracked(Copyable):
    var name: String

    def __init__(out self, name: String):
        self.name = name

    def __deinit__(deinit self):
        print("del", self.name)

@fieldwise_init
struct Box[T: Copyable & Deinitable](Copyable):
    var v: Self.T

    def reset(mut self):
        comptime if Self.T == Tracked:
            rebind[Tracked](self.v) = Tracked("reset")

def put[T: Copyable & Deinitable](mut x: T):
    comptime if T == Int:
        rebind[Int](x) = 7
    elif T == Tracked:
        rebind[Tracked](x) = Tracked("param")

def put_field[T: Copyable & Deinitable](mut b: Box[T]):
    comptime if T == Tracked:
        rebind[Tracked](b.v) = Tracked("field")

def put_through_ref[T: Copyable & Deinitable](mut x: T):
    ref r = x
    comptime if T == Tracked:
        rebind[Tracked](r) = Tracked("ref")

def main():
    var v = 3
    put[Int](v)
    print(v)
    var t = Tracked("local")
    put[Tracked](t)
    print(t.name)
    put_through_ref[Tracked](t)
    print(t.name)
    var b = Box[Tracked](Tracked("box"))
    b.reset()
    print(b.v.name)
    put_field[Tracked](b)
    print(b.v.name)
