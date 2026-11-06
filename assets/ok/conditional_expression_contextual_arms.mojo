# Under a contextual type each arm of a conditional expression converts to
# it when every arm does, as at the pin; otherwise the arms merge.
struct Meters:
    var value: Float64

    @implicit
    def __init__(out self, value: Float64):
        self.value = value

    @implicit
    def __init__(out self, value: String):
        self.value = Float64(value.byte_length())


struct Holder:
    var x: Float32

    def __init__(out self):
        self.x = 0.0


def pick(c: Bool) -> Float32:
    return 0.5 if c else 0.25


def scaled(x: Float32 = 0.5 if True else 0.25) -> Float32:
    return x * 2


def main():
    var c = True
    var d = False
    var label: Optional[String] = String("set") if d else None
    print(label.__bool__())
    var other: Optional[String] = None if d else String("set")
    print(other.value())
    var pair: Tuple[Optional[String], Int] = (String("a") if d else None, 1)
    print(pair[0].__bool__(), pair[1])
    var m: Meters = Float64(1.5) if c else String("four")
    print(m.value)
    var n: Meters = Float64(1.5) if d else String("four")
    print(n.value)
    var z: Int32 = 1 if True else 0.25
    print(z)
    var w: Float32 = 0.5 if c else 0.25
    w = 0.125 if d else 0.75
    print(w)
    var h = Holder()
    h.x = 0.5 if c else 0.75
    print(h.x)
    var l: List[Float32] = [1.0, 2.0]
    l[1] = 0.5 if d else 0.25
    print(l[1])
    print(pick(False), scaled())
