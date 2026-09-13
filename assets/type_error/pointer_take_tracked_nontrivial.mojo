# expect: requires a trivially destructible element
# Taking a pointee that owns storage through an origin-bearing Pointer would
# leave that storage to be destroyed again by its checked owner. The pinned
# Mojo accepts it (docs/non-goals.md).
def main():
    var s = String("owned")
    var p = Pointer(to=s)
    var v = p.unsafe_take_pointee()
    print(v)
