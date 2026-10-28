# A leading-dot bracket argument names a member of its parameter's declared
# type, for a function and for a method alike, as at the pin.


def name_of[d: DType]() -> String:
    return String(d)


struct Tagger:
    def __init__(out self):
        pass

    def tag[d: DType](self) -> String:
        return "dtype " + String(d)


def lanes(v: SIMD) -> Int:
    return v.length


def main():
    print(name_of[.bool]())
    print(Tagger().tag[.int8]())
    var c = SIMD[DType.int32, 4](0, 0, 1, 0)
    print(c.cast[.bool]())
    print(c.cast[.float32]())
    print(lanes(c), lanes(Float64(1.5)))
