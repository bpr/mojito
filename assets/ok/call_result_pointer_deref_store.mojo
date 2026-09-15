# A whole store through the dereferenced pointer field of a view a call
# returns: the temporary view materializes as a hidden owned slot, and the
# store projects through its pointer to the caller's storage.
@fieldwise_init
struct Box:
    var v: Int

@fieldwise_init
struct P[m: Bool, //, o: Origin[mut=m]]:
    var src: Pointer[Box, Self.o]

def make(mut b: Box) -> P[origin_of(b)]:
    return P(Pointer(to=b))

def main():
    var b = Box(7)
    make(b).src[] = Box(11)
    print(b.v)
