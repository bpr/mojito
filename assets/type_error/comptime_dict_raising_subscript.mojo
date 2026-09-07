# expect: cannot call raising function in comptime initializer
# A comptime initializer cannot call a raising function: `Dict.__getitem__`
# raises on a missing key, so a compile-time dictionary is read with `get`.
comptime M = {"a": 1, "b": 2}

def main():
    comptime v = M["a"]
    print(v)
