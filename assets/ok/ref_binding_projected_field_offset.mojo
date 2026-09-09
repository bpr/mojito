# A local `ref` binding to a struct field that does not sit at offset zero:
# reads through the binding address the field once (the binding's handle
# already designates the field), in a method and at the top level alike.
@fieldwise_init
struct Box:
    var pad: Int
    var xs: List[Int]

    def first(self) -> Int:
        ref s = self.xs
        return s[0]

def main():
    var xs = List[Int]()
    xs.append(9)
    xs.append(5)
    var b = Box(1, xs^)
    print(b.first())
    ref inner = b.xs
    print(inner[1])
