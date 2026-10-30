# A `!kgen.pointer` is the address a `Pointer` wraps, not a `Pointer`: it
# has no dereference.
# expect: is not a pointer
def main():
    var x = 7
    var k = Pointer(to=x)._get_kgen_pointer()
    print(k[])
