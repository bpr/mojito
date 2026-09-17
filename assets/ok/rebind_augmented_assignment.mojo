# `rebind[Dest](place) OP= value` writes through the rebound place: an
# operand whose declared type is not `TrivialRegisterPassable` selects
# upstream's `ref[src]` overload, so the target is the operand itself.
@fieldwise_init
struct Box[T: Copyable & Deinitable](Copyable):
    var v: Self.T

    def bump(mut self):
        comptime if Self.T == Int:
            rebind[Int](self.v) += 1

def bump[T: Copyable](mut x: T):
    comptime if T == Int:
        rebind[Int](x) += 1
    elif T == String:
        rebind[String](x) += "!"

def scale[T: Copyable](mut x: T, factor: T):
    comptime if T == Int:
        rebind[Int](x) *= rebind[Int](factor)

def bump_field[T: Copyable & Deinitable](mut b: Box[T]):
    comptime if T == Int:
        rebind[Int](b.v) -= 10

def main():
    var v = 3
    bump[Int](v)
    print(v)
    var s = String("a")
    bump[String](s)
    print(s)
    scale[Int](v, 5)
    print(v)
    var b = Box[Int](7)
    b.bump()
    bump_field[Int](b)
    print(b.v)
