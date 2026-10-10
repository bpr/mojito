# A write through a `Pointer` element of a local tuple reaches its pointee.
def main():
    var left = 4
    var right = 8
    var t = (Pointer(to=left), Pointer(to=right))
    t[1][] += 2
    print(t[0][], t[1][])
