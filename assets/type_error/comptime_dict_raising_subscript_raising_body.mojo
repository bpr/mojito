# expect: cannot call raising function in comptime initializer
# A raising body does not handle a raise in a compile-time initializer:
# `Dict.__getitem__` over a module dictionary constant still cannot run there.
comptime M = {"a": 1, "b": 2}

def main() raises:
    comptime v = M["a"]
    print(v)
