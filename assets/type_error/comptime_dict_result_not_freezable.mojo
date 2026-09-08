# expect: cannot cross back from VM CTFE
# A compile-time expression that produces a pointer-backed value (a `Dict`
# copy) runs in the VM but has no compile-time form to bind; upstream binds
# it, so this is a recorded subset limit.
comptime M = {"a": 1, "b": 2}

def main():
    comptime C = M.copy()
    print(comptime(len(C)))
