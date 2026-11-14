# expect: unsafe_bitcast
# `Pointer.unsafe_bitcast[U]()` takes its target pointee as a parameter
# and no run-time argument.
def main():
    var x: UInt8 = 200
    var p = Pointer(to=x)
    print(p.unsafe_bitcast[Int8](1)[])
