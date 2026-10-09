@fieldwise_init
struct Slot(Copyable, Movable):
    var o: Optional[Int]
    var n: Int

@fieldwise_init
struct Rec(Copyable, Movable, ImplicitlyCopyable):
    var name: String
    var o: Optional[Int]

def slot() -> Slot:
    return Slot(Optional[Int](9), 2)

def record() -> Rec:
    return Rec("x", Optional[Int](4))

def main():
    comptime s = slot()
    comptime r = record()
    print(s.o.value(), s.n)
    print(r.name, r.o.value())
