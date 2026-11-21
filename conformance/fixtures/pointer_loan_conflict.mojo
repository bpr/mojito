# Strict-subset gap: Mojo permits mutating storage a Pointer still
# designates; Mojito attaches an owner loan to the inferred place origin and
# rejects the overlapping write while the pointer is live.
def main():
    var x = 1
    var p = Pointer(to=x)
    x = 5
    print(p[0])
