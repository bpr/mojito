# Explicit method-level value-parameter bindings on non-pack structs
# (`widget.pick[3]()`), including a value-parameterized method on an
# origin-parameterized struct, with and without the default.
@fieldwise_init
struct Widget:
    def pick[n: Int](self) -> Int:
        return n
    def scaled[factor: Int = 2](self, value: Int) -> Int:
        return value * factor

struct Holder[origin: Origin[mut=False]](Movable):
    var target: Pointer[Int, Self.origin]
    def __init__(out self, ref [Self.origin] value: Int):
        self.target = Pointer(to=value)
    def pick[n: Int = 2](self) -> Int:
        return self.target[] + n

def main():
    var widget = Widget()
    print(widget.pick[3](), widget.pick[5]())
    print(widget.scaled(4), widget.scaled[10](4))
    var value = 40
    var holder = Holder(value)
    print(holder.pick(), holder.pick[3]())
