# IntLiteral value types: comptime constants live in literal-typed storage
# (exact i64 in the native backend), flow through comptime branch selection,
# and materialize into scalars and sized lanes with the VM's wrapping rules.
comptime WIDTH = 8
comptime SCALE = WIDTH * 3
comptime BYTE_WRAP = 300


def main():
    comptime if WIDTH > 4:
        print("wide")
    else:
        print("narrow")
    print(WIDTH)
    print(SCALE)
    var w = WIDTH + 1
    print(w)
    print(Int(WIDTH))
    print(Float64(WIDTH))
    var f = Float32(WIDTH)
    print(f)
    var b = UInt8(BYTE_WRAP) # materialization wraps: 44
    print(b)
    print(Int8(BYTE_WRAP)) # wraps: 44
    comptime counter: Int = 1
    print(counter)
