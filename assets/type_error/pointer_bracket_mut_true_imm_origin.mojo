# `mut=True` passes the origin as a `MutOrigin`, which an immutable origin
# does not convert to.
# expect: value passed to 'origin'
def main():
    var x = 5
    var p = Pointer(to=x)
    var r: Pointer[mut=True, Int, ImmOrigin(origin_of(x))] = p
    print(r[])
