# The runtime `DType` surface: `print`/`String` write the name and `repr` the
# `DType.<name>` spelling; `==`/`!=`; upstream's `is_*` queries over the dtype
# code's masks; `DType` parameters, returns, struct fields, `List` elements
# and `Dict` keys; hashing as its code; a `[dt: DType]` value parameter
# returned as a value; and a `comptime` dtype binding read at run time.
def takes(d: DType) -> Bool:
    return d.is_floating_point()

def gives[dt: DType]() -> DType:
    return dt

def pick(flag: Bool) -> DType:
    if flag:
        return DType.int8
    return DType.uint64

struct Holder(Copyable, Movable, Writable):
    var d: DType
    def __init__(out self, d: DType):
        self.d = d
    def write_to(self, mut writer: Some[Writer]):
        writer.write("Holder(", self.d, ")")

def main() raises:
    var x = DType.float32
    print(x)
    print(repr(x))
    print(String(x))
    print(x == DType.float32, x != DType.float32, x == DType.int)
    print(x.is_integral(), x.is_floating_point(), x.is_signed(), x.is_unsigned(), x.is_numeric())
    var y = pick(False)
    print(y, y.is_unsigned(), y.is_signed(), takes(y))
    print(gives[DType.bool](), DType.bool.is_numeric(), DType.int.is_integral())
    var h = Holder(DType.uint8)
    print(h)
    var ds: List[DType] = [DType.int, DType.float64]
    print(len(ds), ds[1])
    print(hash(DType.int) == hash(DType.int))
    x = DType.bool
    print(x, x.is_half_float(), x.is_float8())
    comptime c = DType.int64
    print(c)
    var d = Dict[DType, Int]()
    d[DType.int] = 3
    print(d[DType.int])
