# A mutable origin-bearing Pointer may take a trivially destructible pointee.
def main():
    var x = 1
    var p = Pointer(to=x)
    var v = p.unsafe_take_pointee()
    print(v)
