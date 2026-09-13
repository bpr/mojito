# In-range `Int`/`Float64` conversions, natively and on the VM. The defined
# edges Mojito adds — saturation out of range and `Int(nan) == 0`, where the
# pin's conversion is poison — are the `defined-float-to-int-edges`
# conformance case.
def compute() -> Int:
    var flags = 0
    if Int(Float64(3.7)) == 3:
        flags = flags + 1
    if Int(Float64(-3.7)) == -3:
        flags = flags + 2
    if Float64(7) == 7.0:
        flags = flags + 4
    if Int(Float64(9007199254740992.0)) == 9007199254740992:
        flags = flags + 8
    return flags

def main():
    print(compute())
