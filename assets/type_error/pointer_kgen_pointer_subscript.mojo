# A `!kgen.pointer` is the address a `Pointer` wraps, not a `Pointer`: it
# declares no `__getitem__`, so it has no dereference.
# expect: cannot be indexed
def main():
    var x = 7
    var k = Pointer(to=x)._get_kgen_pointer()
    print(k[])
