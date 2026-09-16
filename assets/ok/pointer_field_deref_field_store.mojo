# A field store through the dereferenced pointer field of a view reaches the
# pointee's field: the place (`p.src[].v`) reaches a stored handle below its
# root, so the write goes through the reference walk rather than raw frame
# storage.
@fieldwise_init
struct Box:
    var v: Int

@fieldwise_init
struct P[m: Bool, //, o: Origin[mut=m]]:
    var src: Pointer[Box, Self.o]

def main():
    var b = Box(7)
    var p = P(Pointer(to=b))
    p.src[].v = 9
    print(b.v)
