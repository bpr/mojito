def make(flag: Bool) -> Optional[Int]:
    if flag:
        return Optional[Int](3)
    return Optional[Int]()

def label() -> Optional[String]:
    return Optional[String]("hi")

def get[o: Optional[Int]]() -> Int:
    return o.or_else(0)

def main():
    comptime empty = make(False)
    comptime full = make(True)
    comptime bare = Optional[Int](7)
    comptime value = full.value()
    comptime text = label()
    var copy = full
    print(empty.or_else(5), full.value(), Bool(empty), Bool(full))
    print(bare.value(), value, get[full](), copy.value(), text.value())
